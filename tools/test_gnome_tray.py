#!/usr/bin/env python3
"""Exercise packaged tray handlers with virtual clicks in a private headless GNOME.

The disposable Shell, bus, home, virtual monitor and pointer belong to this test.
The application uses synthetic data and starts no device or network services.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
UUID = 'desk-switch@siarheisinelnikau'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--private-bus', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not args.private_bus:
        return subprocess.call(['dbus-run-session', '--', sys.executable, __file__, '--private-bus',
            '--binary', str(args.binary.resolve()), '--out', str(args.out.resolve())],
            env=dict(os.environ, DESK_SWITCH_GNOME_PARENT_BUS=os.environ.get('DBUS_SESSION_BUS_ADDRESS', '')))
    assert os.environ['DBUS_SESSION_BUS_ADDRESS'] != os.environ.get('DESK_SWITCH_GNOME_PARENT_BUS')
    from gi.repository import Gio, GLib
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    args.out.mkdir(parents=True, exist_ok=True)

    def call(name, path, interface, method, signature, parameters):
        return bus.call_sync(name, path, interface, method, GLib.Variant(signature, parameters),
            None, Gio.DBusCallFlags.NONE, 3000, None).unpack()

    def shell(code):
        success, value = call('org.gnome.Shell', '/org/gnome/Shell', 'org.gnome.Shell', 'Eval', '(s)', (code,))
        assert success, value
        return json.loads(value) if value else None

    def extension(method, uuid):
        return call('org.gnome.Shell.Extensions', '/org/gnome/Shell/Extensions',
            'org.gnome.Shell.Extensions', method, '(s)', (uuid,))

    def wait_for(predicate, message, timeout=15):
        until = time.monotonic() + timeout
        last = None
        while time.monotonic() < until:
            try:
                value = predicate()
                if value:
                    return value
            except (GLib.Error, AssertionError) as error:
                last = str(error)
            time.sleep(0.1)
        raise AssertionError(f'{message}: {last}')

    def stop(process):
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=4)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=4)

    app = compositor = None
    with tempfile.TemporaryDirectory(prefix='desk-switch-gnome-') as temporary:
        home = Path(temporary)
        runtime = home / 'runtime'
        runtime.mkdir(mode=0o700)
        data = home / 'data'
        shutil.copytree(ROOT / 'packaging/linux/gnome-extension', data / 'gnome-shell/extensions' / UUID)
        env = dict(os.environ, HOME=str(home), XDG_RUNTIME_DIR=str(runtime),
            XDG_CONFIG_HOME=str(home / 'config'), XDG_CACHE_HOME=str(home / 'cache'),
            XDG_DATA_HOME=str(data), XDG_CURRENT_DESKTOP='GNOME', XDG_SESSION_TYPE='wayland',
            GSETTINGS_BACKEND='keyfile', LIBGL_ALWAYS_SOFTWARE='1')
        for name in ['DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'DESK_SWITCH_CAPTURE_TO', 'DESK_SWITCH_RENDER_PROBE']:
            env.pop(name, None)
        try:
            # Seed this private home before Shell starts. Ubuntu's default
            # enabled-extension list otherwise races our explicit Enable calls
            # and can export two StatusNotifierWatchers during initialization.
            for key, value in [('enabled-extensions', '[]'), ('disabled-extensions', '[]'),
                               ('disable-user-extensions', 'false')]:
                subprocess.run(['gsettings', 'set', 'org.gnome.shell', key, value],
                    env=env, check=True, timeout=3, capture_output=True)
            with (args.out / 'shell.log').open('w') as log:
                compositor = subprocess.Popen(['gnome-shell', '--headless', '--wayland', '--unsafe-mode',
                    '--virtual-monitor=1920x1080', '--wayland-display=desk-switch-test'],
                    env=env, stdout=log, stderr=log)
                wait_for(lambda: shell('!Main.layoutManager._startingUp'), 'The private GNOME did not start', 30)
                wait_for(lambda: shell('Boolean(Main.extensionManager._initializationPromise)'),
                    'The private extension loader did not start')
                shell('(async () => { await Main.extensionManager._initializationPromise; return true; })()')
                shell('Main.overview.hide(); true')
                available = call('org.gnome.Shell.Extensions', '/org/gnome/Shell/Extensions',
                    'org.gnome.Shell.Extensions', 'ListExtensions', '()', ())[0]
                host = next((uuid for uuid in ['ubuntu-appindicators@ubuntu.com', 'appindicatorsupport@rgcjonas.gmail.com']
                    if uuid in available), None)
                assert host, 'Install gnome-shell-extension-appindicator for this isolated test'
                assert extension('EnableExtension', host) == (True,)
                wait_for(lambda: extension('GetExtensionInfo', host)[0].get('state') == 1,
                    'The AppIndicator extension did not finish enabling')
                assert extension('EnableExtension', UUID) == (True,)
                wait_for(lambda: extension('GetExtensionInfo', UUID)[0].get('state') == 1,
                    'The tray companion did not finish enabling')
                wait_for(lambda: call('org.freedesktop.DBus', '/org/freedesktop/DBus',
                    'org.freedesktop.DBus', 'NameHasOwner', '(s)', ('org.kde.StatusNotifierWatcher',))[0],
                    'The native tray watcher did not acquire its private bus name')
                native_env = shell("({DISPLAY: GLib.getenv('DISPLAY'), XAUTHORITY: GLib.getenv('XAUTHORITY'), WAYLAND_DISPLAY: GLib.getenv('WAYLAND_DISPLAY')})")
                assert native_env.get('DISPLAY'), 'The private GNOME must provide its own XWayland display'
                env.update({name: value for name, value in native_env.items() if value})
                env['DESK_SWITCH_PREVIEW_LANGUAGE'] = 'en'
                with (args.out / 'application.log').open('w') as app_log:
                    app = subprocess.Popen([str(args.binary.resolve()), 'gui-preview', 'shared-four',
                        '--state-dir', str(home / 'profile')], env=env, stdout=app_log, stderr=app_log)
                    icon = "Object.values(Main.panel.statusArea).find(i => i._indicator?.id === 'desk-switch')"
                    wait_for(lambda: shell(f'Boolean(({icon})?.visible)'), 'The actual AppIndicator did not appear')
                    shell(f'global.deskSwitchTestIcon = () => {icon}; true')
                    pid = app.pid

                    def visible(title):
                        return shell(f'global.get_window_actors().some(a => a.meta_window.get_pid() === {pid} && '
                            f'a.meta_window.get_title() === {json.dumps(title)} && a.visible)')

                    wait_for(lambda: visible('Desk Switch'), 'The native main window is missing')
                    shell('Main.overview.hide(); true')
                    wait_for(lambda: shell('!Main.layoutManager._startingUp && !Main.overview.visible && '
                        'global.deskSwitchTestIcon()?.mapped && global.deskSwitchTestIcon().width > 0 && '
                        'global.deskSwitchTestIcon().height > 0'), 'The panel button is not ready for physical hit testing')
                    shell("(async () => { const Clutter = (await import('gi://Clutter')).default; "
                        "global.deskSwitchTestPointer = global.stage.context.get_backend().get_default_seat().create_virtual_device(Clutter.InputDeviceType.POINTER_DEVICE); return true; })()")

                    def click(button):
                        # Virtual seat requests are asynchronous. Sending motion,
                        # press and release in one Eval can click before the new
                        # device reaches the indicator, especially during login.
                        wait_for(lambda: shell("(async () => { const Clutter = (await import('gi://Clutter')).default; "
                            "const icon = global.deskSwitchTestIcon(); if (!icon?.mapped || icon.width <= 0 || icon.height <= 0) return false; "
                            "let [x, y] = icon.get_transformed_position(); x += icon.width / 2; y += icon.height / 2; "
                            "let [px, py] = global.get_pointer(); "
                            "if (Math.abs(px - x) > 1 || Math.abs(py - y) > 1) { "
                            "global.deskSwitchTestPointer.notify_absolute_motion(GLib.get_monotonic_time(), x, y); return false; } "
                            "for (let actor = global.stage.get_actor_at_pos(Clutter.PickMode.REACTIVE, px, py); actor; actor = actor.get_parent()) "
                            "if (actor === icon) return true; return false; })()"),
                            'The virtual pointer did not reach the actual panel button')
                        shell("(async () => { const Clutter = (await import('gi://Clutter')).default; "
                            f"global.deskSwitchTestPointer.notify_button(GLib.get_monotonic_time(), {button}, Clutter.ButtonState.PRESSED); return true; }})()")
                        mask = 1 << (button + 7)
                        wait_for(lambda: shell(f'(global.get_pointer()[2] & {mask}) !== 0'), 'The virtual button press was not delivered')
                        shell("(async () => { const Clutter = (await import('gi://Clutter')).default; "
                            f"global.deskSwitchTestPointer.notify_button(GLib.get_monotonic_time(), {button}, Clutter.ButtonState.RELEASED); return true; }})()")
                        wait_for(lambda: shell(f'(global.get_pointer()[2] & {mask}) === 0'), 'The virtual button release was not delivered')

                    for _ in range(3):
                        click(1)
                        wait_for(lambda: visible('Desk Switch quick controls'), 'A completed left-click did not open the device popup')
                        assert shell('global.deskSwitchTestIcon().menu.isOpen') is False, 'Left-click opened the native menu'
                        click(1)
                        wait_for(lambda: not visible('Desk Switch quick controls'), 'A second left-click did not dismiss the popup')
                        click(3)
                        wait_for(lambda: shell('global.deskSwitchTestIcon().menu.isOpen'), 'Right-click did not open the native menu')
                        shell('global.deskSwitchTestIcon().menu.close(); true')
                    shell(f"global.get_window_actors().find(a => a.meta_window.get_pid() === {pid} && a.meta_window.get_title() === 'Desk Switch').meta_window.delete(global.get_current_time()); true")
                    wait_for(lambda: not visible('Desk Switch'), 'Close did not leave the main window hidden')
                    click(1)
                    wait_for(lambda: visible('Desk Switch quick controls'), 'Left-click did not open the popup while the main window was hidden')
                    assert not visible('Desk Switch'), 'Opening the popup also restored the main window'
                    assert app.poll() is None
                    version = subprocess.check_output(['gnome-shell', '--version'], env=env, text=True, timeout=3).strip()
                    (args.out / 'result.json').write_text(json.dumps({'gnome': version, 'virtual_click_cycles': 3,
                        'left_click_device_popup': True, 'right_click_native_menu': True, 'hidden_root_popup': True,
                        'scope': 'Actual GNOME/AppIndicator/packaged companion on a private headless compositor; synthetic application and virtual pointer only.'}, indent=2) + '\n')
                    print('PASS: actual GNOME primary clicks open the device popup, secondary clicks open its native menu, and a hidden main window stays hidden.')
        finally:
            if compositor is not None and compositor.poll() is None:
                try:
                    diagnostics = shell("(() => { const icon = global.deskSwitchTestIcon?.(); return "
                        "{pointer: global.get_pointer(), overview: Main.overview.visible, "
                        "icon: icon ? {mapped: icon.mapped, position: icon.get_transformed_position(), "
                        "size: [icon.width, icon.height], menu: icon.menu.isOpen} : null, "
                        "windows: global.get_window_actors().map(a => ({pid: a.meta_window.get_pid(), title: a.meta_window.get_title(), visible: a.visible}))}; })()")
                    (args.out / 'diagnostics.json').write_text(json.dumps(diagnostics, indent=2) + '\n')
                except (GLib.Error, AssertionError):
                    pass
            stop(app)
            stop(compositor)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
