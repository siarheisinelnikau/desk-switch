#!/usr/bin/env python3
"""Bounded real-window tray lifecycle checks on a private Xvfb and D-Bus.

Uses the application's synthetic preview: no device services, network discovery,
clipboard, real desktop, monitor commands or user settings are involved.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--private-bus', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not args.private_bus:
        env = dict(os.environ, DESK_SWITCH_TEST_PARENT_BUS=os.environ.get('DBUS_SESSION_BUS_ADDRESS', ''))
        return subprocess.call(['dbus-run-session', '--', sys.executable, __file__, '--private-bus',
                                '--binary', str(args.binary.resolve()), '--out', str(args.out.resolve())], env=env)
    assert os.environ['DBUS_SESSION_BUS_ADDRESS'] != os.environ.get('DESK_SWITCH_TEST_PARENT_BUS')
    from gi.repository import Gio, GLib

    context = GLib.MainContext.default()
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def call(name, path, interface, method, signature, values):
        result = bus.call_sync(name, path, interface, method, GLib.Variant(signature, values),
                               None, Gio.DBusCallFlags.NONE, 2000, None)
        return result.unpack()

    registered = []
    xml = '''<node><interface name="org.kde.StatusNotifierWatcher">
      <method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
      <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
      <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
      <property name="ProtocolVersion" type="i" access="read"/>
    </interface></node>'''

    def method(_bus, _sender, _path, _interface, _method, parameters, invocation):
        registered.append(parameters.unpack()[0])
        invocation.return_value(None)

    def property_(_bus, _sender, _path, _interface, name):
        return {'IsStatusNotifierHostRegistered': GLib.Variant('b', True),
                'RegisteredStatusNotifierItems': GLib.Variant('as', registered),
                'ProtocolVersion': GLib.Variant('i', 0)}[name]

    registration = bus.register_object('/StatusNotifierWatcher',
                                       Gio.DBusNodeInfo.new_for_xml(xml).interfaces[0], method, property_, None)
    assert call('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
                'RequestName', '(su)', ('org.kde.StatusNotifierWatcher', 4)) == (1,)

    def wait_for(predicate, message, timeout=10):
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            while context.pending():
                context.iteration(False)
            value = predicate()
            if value:
                return value
            time.sleep(0.05)
        raise AssertionError(message)

    def command(*arguments):
        return subprocess.check_output(arguments, text=True, timeout=3, stderr=subprocess.DEVNULL)

    def window(pid, title):
        for line in command('wmctrl', '-lp').splitlines():
            parts = line.split(None, 4)
            if len(parts) == 5 and parts[2] == str(pid) and parts[4] == title:
                return parts[0]
        return None

    def visible(identifier):
        try:
            return 'Map State: IsViewable' in command('xwininfo', '-id', identifier)
        except subprocess.CalledProcessError:
            return False

    def stop(process):
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)

    args.out.mkdir(parents=True, exist_ok=True)
    manager = subprocess.Popen(['openbox', '--sm-disable'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    process = None
    try:
        wait_for(lambda: '_NET_SUPPORTING_WM_CHECK(WINDOW)' in command('xprop', '-root', '_NET_SUPPORTING_WM_CHECK'),
                 'The isolated window manager did not start')
        with tempfile.TemporaryDirectory(prefix='desk-switch-tray-') as temporary:
            state = Path(temporary)
            env = dict(os.environ, LIBGL_ALWAYS_SOFTWARE='1', DESK_SWITCH_PREVIEW_LANGUAGE='en')
            env.pop('DESK_SWITCH_CAPTURE_TO', None)
            # A present but unusable Wayland socket must not override X11 for
            # this tray UI. The input backend is not started by the fixture.
            env['WAYLAND_DISPLAY'] = 'desk-switch-unused-test-socket'
            probe = args.out.resolve() / 'render.json'
            with (args.out / 'render.log').open('w') as log:
                process = subprocess.Popen([str(args.binary.resolve()), 'gui-preview', 'shared-four', '--state-dir', str(state / 'render')],
                                           env=dict(env, DESK_SWITCH_RENDER_PROBE=str(probe)), stdout=log, stderr=log)
                wait_for(lambda: process.poll() is not None, 'The rendering probe exceeded its deadline', 45)
                assert process.returncode == 0, (args.out / 'render.log').read_text()
            rendering = json.loads(probe.read_text())
            assert 'llvmpipe' in rendering['backend'].lower(), rendering['backend']
            for sample in rendering['samples']:
                assert sample['settled'] and sample['ui_frames'] == sample['popup_ui_frames'] == sample['input_events'] == 0, sample
            registered.clear()
            with (args.out / 'lifecycle.log').open('w') as log:
                process = subprocess.Popen([str(args.binary.resolve()), 'gui-preview', 'shared-four', '--state-dir', str(state / 'lifecycle')],
                                           env=env, stdout=log, stderr=log)
                name = wait_for(lambda: next((item for item in registered if item.startswith(f'org.kde.StatusNotifierItem-{process.pid}-')), None),
                                'The actual ksni tray did not register')
                root = wait_for(lambda: window(process.pid, 'Desk Switch'), 'The main window did not appear')
                wait_for(lambda: visible(root), 'The main window is not mapped')
                properties = call(name, '/StatusNotifierItem', 'org.freedesktop.DBus.Properties', 'GetAll', '(s)', ('org.kde.StatusNotifierItem',))[0]
                assert properties['Id'] == 'desk-switch' and properties['ItemIsMenu'] is False
                _, layout = call(name, '/MenuBar', 'com.canonical.dbusmenu', 'GetLayout', '(iias)', (0, -1, ['label']))
                entries = {child[1]['label']: child[0] for child in layout[2] if 'label' in child[1]}

                def menu(label):
                    call(name, '/MenuBar', 'com.canonical.dbusmenu', 'Event', '(isvu)',
                         (entries[label], 'clicked', GLib.Variant('i', 0), 0))

                for _ in range(2):
                    subprocess.run(['wmctrl', '-ic', root], check=True, timeout=3)
                    wait_for(lambda: not visible(root), 'Close X did not hide the actual main window')
                    assert process.poll() is None, 'Close X exited instead of keeping the tray alive'
                    menu('Open Desk Switch')
                    wait_for(lambda: visible(root), 'Open did not restore the same main window')
                subprocess.run(['wmctrl', '-ic', root], check=True, timeout=3)
                wait_for(lambda: not visible(root), 'The main window did not hide before popup testing')
                for _ in range(2):
                    call(name, '/StatusNotifierItem', 'org.kde.StatusNotifierItem', 'Activate', '(ii)', (110, 24))
                    popup = wait_for(lambda: window(process.pid, 'Desk Switch quick controls'), 'Activate did not open quick controls')
                    wait_for(lambda: visible(popup), 'Quick controls are not mapped')
                    assert not visible(root), 'Quick controls also restored the main window'
                    call(name, '/StatusNotifierItem', 'org.kde.StatusNotifierItem', 'Activate', '(ii)', (110, 24))
                    wait_for(lambda: not visible(popup), 'The repeated click did not dismiss quick controls')
                menu('Quit Desk Switch')
                wait_for(lambda: process.poll() is not None, 'Quit did not exit while the main window was hidden')
                assert process.returncode == 0, (args.out / 'lifecycle.log').read_text()
            (args.out / 'lifecycle.json').write_text(json.dumps({
                'backend': rendering['backend'], 'native_window_system': 'X11', 'root_id': root,
                'close_restore_cycles': 2, 'popup_cycles_while_main_hidden': 2, 'quit_while_hidden': True,
                'scope': 'Actual application fixture on Xvfb/private D-Bus; no physical GNOME mouse routing or real user devices.'}, indent=2) + '\n')
        print('PASS: real X11 close/hide/restore, direct SNI popup activation, native menu export, hidden quit and settled rendering. Physical GNOME clicks remain untested.')
    finally:
        if process is not None:
            stop(process)
        stop(manager)
        bus.unregister_object(registration)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
