#!/usr/bin/env python3
"""Exercise real portal requests on a fresh D-Bus, without compositor or input."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parents[1] / 'target/debug/desk-switch')
    parser.add_argument('--private-bus', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not args.private_bus:
        env = dict(os.environ, DESK_SWITCH_TEST_PARENT_BUS=os.environ.get('DBUS_SESSION_BUS_ADDRESS', ''))
        return subprocess.call(['dbus-run-session', '--', sys.executable, __file__, '--private-bus', '--binary', str(args.binary.resolve())], env=env)
    assert os.environ.get('DBUS_SESSION_BUS_ADDRESS')
    assert os.environ['DBUS_SESSION_BUS_ADDRESS'] != os.environ.get('DESK_SWITCH_TEST_PARENT_BUS')
    from gi.repository import Gio, GLib

    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    result = bus.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus', 'RequestName', GLib.Variant('(su)', ('org.freedesktop.portal.Desktop', 4)), None, Gio.DBusCallFlags.NONE, 2000, None)
    assert result.unpack() == (1,), 'The private bus must not already have a portal'
    context = GLib.MainContext.default()
    requests, closed, registrations = [], [], []
    generation = 0
    cancel_start = False
    root = '/org/freedesktop/portal/desktop'
    xml = '''<node><interface name="org.freedesktop.portal.RemoteDesktop">
      <property name="version" type="u" access="read"/><property name="AvailableDeviceTypes" type="u" access="read"/>
      <method name="CreateSession"><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
      <method name="SelectDevices"><arg type="o" direction="in"/><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
      <method name="Start"><arg type="o" direction="in"/><arg type="s" direction="in"/><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
      <method name="ConnectToEIS"><arg type="o" direction="in"/><arg type="a{sv}" direction="in"/><arg type="h" direction="out"/></method>
    </interface></node>'''
    session_xml = '<node><interface name="org.freedesktop.portal.Session"><method name="Close"/></interface></node>'

    def session_method(conn, sender, path, interface, name, params, invocation):
        closed.append(path)
        invocation.return_value(GLib.Variant('()', ()))

    def method(conn, sender, path, interface, name, params, invocation):
        nonlocal generation
        values = params.unpack()
        if name == 'ConnectToEIS':
            # Consent/token handling is complete. Never connect to real EIS.
            invocation.return_dbus_error('org.freedesktop.DBus.Error.Failed', 'Synthetic EIS endpoint intentionally unavailable')
            return
        options = values[-1]
        request = '/org/freedesktop/portal/desktop/request/' + sender[1:].replace('.', '_') + '/' + options['handle_token']
        response, code = {}, 0
        if name == 'CreateSession':
            session = '/org/freedesktop/portal/desktop/session/' + sender[1:].replace('.', '_') + '/' + options['session_handle_token']
            registrations.append(bus.register_object(session, Gio.DBusNodeInfo.new_for_xml(session_xml).interfaces[0], session_method, None, None))
            response['session_handle'] = GLib.Variant('s', session)
        elif name == 'SelectDevices':
            requests.append(options)
        elif name == 'Start':
            if cancel_start:
                code = 1
            else:
                generation += 1
                response = {'devices': GLib.Variant('u', 3), 'restore_token': GLib.Variant('s', f'fixture-permission-{generation}')}
        invocation.return_value(GLib.Variant('(o)', (request,)))
        def respond():
            bus.emit_signal(sender, request, 'org.freedesktop.portal.Request', 'Response', GLib.Variant('(ua{sv})', (code, response)))
            return False
        GLib.timeout_add(15, respond)

    bus.register_object(root, Gio.DBusNodeInfo.new_for_xml(xml).interfaces[0], method, lambda *a: GLib.Variant('u', 2 if a[-1] == 'version' else 3), None)
    with tempfile.TemporaryDirectory(prefix='desk-switch-portal-test-') as directory:
        token = Path(directory) / 'wayland-restore-token'
        env = dict(os.environ, DESK_SWITCH_STATE=directory, XDG_SESSION_TYPE='wayland')
        env.pop('DESK_SWITCH_MANAGED', None)
        for expected in [None, 'fixture-permission-1', 'fixture-permission-2']:
            cancel_start = expected == 'fixture-permission-2'
            child = subprocess.Popen([str(args.binary.resolve()), 'portal-check', '--consent'], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            deadline = time.monotonic() + 15
            try:
                while child.poll() is None and time.monotonic() < deadline:
                    while context.pending():
                        context.iteration(False)
                    time.sleep(.005)
                out, err = child.communicate(timeout=1)
            except BaseException:
                child.kill()
                child.wait()
                raise
            assert child.returncode != 0, 'Only synthetic EIS failure or cancellation should end setup'
            assert len(requests) == (1 if expected is None else int(expected[-1]) + 1), (requests, out, err)
            options = requests[-1]
            assert options['types'] == 3 and options['persist_mode'] == 2, options
            assert options.get('restore_token') == expected, options
            if cancel_start:
                assert not token.exists(), 'A single-use token must not survive cancelled restoration'
            else:
                assert token.read_text() == f'fixture-permission-{generation}'
                assert token.stat().st_mode & 0o777 == 0o600
        assert len(closed) == 3, closed
    print('PASS: actual portal requests persist, privately rotate and consume restore tokens; cancellation closes each session. Private D-Bus only, no input or compositor.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
