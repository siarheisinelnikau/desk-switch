// Adapted from DownloadManager's MIT-licensed GNOME tray companion.
import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import St from 'gi://St';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const INDICATOR_ID = 'desk-switch';
const CLIPBOARD_NAME = 'io.github.siarheisinelnikau.DeskSwitch.Clipboard';
const CLIPBOARD_PATH = '/io/github/siarheisinelnikau/DeskSwitch/Clipboard';
const CLIPBOARD_LIMIT = 8 * 1024 * 1024;
const CLIPBOARD_INTERFACE = `<node><interface name="${CLIPBOARD_NAME}">
    <method name="Version"><arg type="u" direction="out"/></method>
    <method name="Read">
        <arg type="u" direction="in"/>
        <arg type="u" direction="out"/><arg type="s" direction="out"/><arg type="ay" direction="out"/>
    </method>
    <method name="Write">
        <arg type="s" direction="in"/><arg type="ay" direction="in"/>
        <arg type="u" direction="out"/>
    </method>
    <method name="ActivateWindow"><arg type="s" direction="in"/></method>
    <method name="PointerState"><arg type="i" direction="out"/><arg type="i" direction="out"/></method>
</interface></node>`;

export default class DeskSwitchTray extends Extension {
    enable() {
        this._icons = new Map();
        this._pressedIcon = null;
        this._scanSource = 0;
        this._cancellable = new Gio.Cancellable();
        this._alive = true;
        this._allowedCallers = new Map();
        this._focusRequests = new Map();
        this._clipboardSerial = 1;
        this._clipboardRejected = null;
        this._clipboard = St.Clipboard.get_default();
        this._selection = global.display.get_selection();
        this._selectionSignal = this._selection.connect('owner-changed', (_selection, type) => {
            if (type === Meta.SelectionType.SELECTION_CLIPBOARD) {
                this._clipboardSerial = (this._clipboardSerial + 1) >>> 0 || 1;
                this._clipboardRejected = null;
            }
        });
        this._clipboardBridge = Gio.DBusExportedObject.wrapJSObject(CLIPBOARD_INTERFACE, this);
        this._clipboardBridge.export(Gio.DBus.session, CLIPBOARD_PATH);
        this._clipboardOwner = Gio.bus_own_name(Gio.BusType.SESSION, CLIPBOARD_NAME,
            Gio.BusNameOwnerFlags.NONE, null, null, null);
        this._stageCapture = global.stage.connect('captured-event',
            (_stage, event) => this._handleEvent(event));
        this._boxes = [Main.panel._leftBox, Main.panel._centerBox, Main.panel._rightBox]
            .map(box => [box, box.connect('child-added', () => this._queueScan())]);
        this._queueScan();
    }

    _queueScan() {
        if (this._scanSource)
            return;
        this._scanSource = GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
            this._scanSource = 0;
            for (const icon of Object.values(Main.panel.statusArea))
                this._watchIcon(icon);
            return GLib.SOURCE_REMOVE;
        });
    }

    _watchIcon(icon) {
        if (!icon?._indicator?.busName || !icon._indicator._proxy || this._icons.has(icon))
            return;
        const destroy = icon.connect('destroy', () => {
            if (this._pressedIcon === icon)
                this._pressedIcon = null;
            this._icons.delete(icon);
        });
        this._icons.set(icon, destroy);
    }

    _iconAt(x, y) {
        const target = global.stage.get_actor_at_pos(Clutter.PickMode.REACTIVE, x, y);
        if (!target)
            return null;
        for (const icon of this._icons.keys()) {
            if (icon.visible && icon._indicator.id === INDICATOR_ID &&
                (target === icon || icon.contains(target))) {
                const rect = icon.get_transformed_extents();
                if (x >= rect.get_x() && x < rect.get_x() + rect.get_width() &&
                    y >= rect.get_y() && y < rect.get_y() + rect.get_height())
                    return icon;
            }
        }
        return null;
    }

    _handleEvent(event) {
        const type = event.type();
        if ((type !== Clutter.EventType.BUTTON_PRESS &&
            type !== Clutter.EventType.BUTTON_RELEASE) ||
            event.get_button() !== Clutter.BUTTON_PRIMARY)
            return Clutter.EVENT_PROPAGATE;
        const [x, y] = event.get_coords();
        const icon = this._iconAt(x, y);
        if (type === Clutter.EventType.BUTTON_PRESS) {
            this._pressedIcon = icon;
            if (!icon)
                return Clutter.EVENT_PROPAGATE;
            icon.menu.close();
        } else {
            const pressed = this._pressedIcon;
            this._pressedIcon = null;
            if (!pressed)
                return Clutter.EVENT_PROPAGATE;
            if (pressed === icon)
                this._activate(icon._indicator, x, y);
        }
        return Clutter.EVENT_STOP;
    }

    _activate(indicator, x, y) {
        // Call our exported action directly, as DownloadManager's companion
        // does. AppIndicator.open() may open the host menu instead of Activate.
        // Our ksni Activate handler opens quick controls; right clicks continue
        // through the unchanged Shell/AppIndicator context-menu handler.
        Gio.DBus.session.call(
            indicator.busName, indicator._proxy.g_object_path ?? '/StatusNotifierItem',
            'org.kde.StatusNotifierItem', 'Activate',
            new GLib.Variant('(ii)', [Math.round(x), Math.round(y)]),
            null, Gio.DBusCallFlags.NONE, 1500, this._cancellable,
            (connection, result) => {
                try {
                    connection.call_finish(result);
                } catch (error) {
                    if (!error.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                        console.error(`Desk Switch tray activation failed: ${error.message}`);
                }
            });
    }

    VersionAsync(_parameters, invocation) {
        invocation.return_value(new GLib.Variant('(u)', [1]));
    }

    _processId(name) {
        return this._connectionNumber(name, 'GetConnectionUnixProcessID');
    }

    _userId(name) {
        return this._connectionNumber(name, 'GetConnectionUnixUser');
    }

    _connectionNumber(name, method) {
        return new Promise((resolve, reject) => {
            Gio.DBus.session.call('org.freedesktop.DBus', '/org/freedesktop/DBus',
                'org.freedesktop.DBus', method,
                new GLib.Variant('(s)', [name]), null, Gio.DBusCallFlags.NONE,
                1000, this._cancellable, (connection, result) => {
                    try {
                        resolve(connection.call_finish(result).deep_unpack()[0]);
                    } catch (error) {
                        reject(error);
                    }
                });
        });
    }

    async _authorizeCaller(invocation) {
        if (!this._alive)
            throw new Error('The clipboard companion is disabled');
        const sender = invocation.get_sender();
        if (this._allowedCallers.has(sender))
            return this._allowedCallers.get(sender);
        const indicators = [...this._icons.keys()].filter(icon =>
            icon._indicator.id === INDICATOR_ID && icon._indicator.busName);
        const callerPid = await this._processId(sender);
        const indicatorPids = await Promise.all(indicators.map(icon =>
            this._processId(icon._indicator.busName).catch(() => 0)));
        if (!this._alive || callerPid <= 0 || !indicatorPids.includes(callerPid))
            throw new Error('Only the running Desk Switch process may use this clipboard bridge');
        if (this._allowedCallers.size > 64)
            this._allowedCallers.clear();
        this._allowedCallers.set(sender, callerPid);
        return callerPid;
    }

    async _boundedText(path, limit) {
        const file = Gio.File.new_for_path(path);
        const stream = await new Promise((resolve, reject) => {
            file.read_async(GLib.PRIORITY_DEFAULT, this._cancellable, (source, result) => {
                try { resolve(source.read_finish(result)); } catch (error) { reject(error); }
            });
        });
        const parts = [];
        let length = 0;
        try {
            for (;;) {
                const bytes = await new Promise((resolve, reject) => {
                    stream.read_bytes_async(Math.min(4096, limit + 1 - length),
                        GLib.PRIORITY_DEFAULT, this._cancellable, (source, result) => {
                            try { resolve(source.read_bytes_finish(result)); } catch (error) { reject(error); }
                        });
                });
                const data = bytes.get_data();
                if (data.length === 0)
                    break;
                length += data.length;
                if (length > limit)
                    throw new Error('Process metadata exceeds its limit');
                parts.push(Uint8Array.from(data));
            }
        } finally {
            stream.close(null);
        }
        const data = new Uint8Array(length);
        let offset = 0;
        for (const part of parts) {
            data.set(part, offset);
            offset += part.length;
        }
        return new TextDecoder('utf-8', {fatal: true}).decode(data);
    }

    _fileInfo(path, attributes) {
        const file = Gio.File.new_for_path(path);
        return new Promise((resolve, reject) => {
            file.query_info_async(attributes, Gio.FileQueryInfoFlags.NOFOLLOW_SYMLINKS,
                GLib.PRIORITY_DEFAULT, this._cancellable, (source, result) => {
                    try { resolve(source.query_info_finish(result)); } catch (error) { reject(error); }
                });
        });
    }

    async _processIdentity(pid) {
        if (!Number.isInteger(pid) || pid <= 0)
            throw new Error('Invalid process identity');
        const status = await this._boundedText(`/proc/${pid}/status`, 16384);
        const stat = await this._boundedText(`/proc/${pid}/stat`, 4096);
        const recordedPid = status.match(/^Pid:\s*(\d+)$/m)?.[1];
        const parent = status.match(/^PPid:\s*(\d+)$/m)?.[1];
        const users = status.match(/^Uid:\s*(\d+)\s+(\d+)\s+(\d+)\s+(\d+)$/m)?.slice(1);
        const end = stat.lastIndexOf(')');
        const fields = stat.slice(end + 1).trim().split(/\s+/);
        if (Number(recordedPid) !== pid || !parent || !users ||
            !stat.startsWith(`${pid} (`) || end < 0 || fields.length < 20 ||
            !/^[A-Za-z]$/.test(fields[0]) || ['Z', 'X', 'x'].includes(fields[0]) ||
            fields[1] !== parent || !/^\d+$/.test(fields[19]))
            throw new Error('Process metadata is incomplete');
        return {pid, parent: Number(parent), users: users.map(Number), started: fields[19]};
    }

    async _executable(pid) {
        const info = await this._fileInfo(`/proc/${pid}/exe`, 'standard::symlink-target');
        const target = info.get_symlink_target();
        if (!target || target.length > 4096 || !GLib.path_is_absolute(target))
            throw new Error('Process executable is unavailable');
        return target;
    }

    async _authorizePointer(invocation) {
        try {
            return await this._authorizeCaller(invocation);
        } catch {
            // Clipboard and window activation keep their original exact GUI PID
            // policy. Pointer observation alone also accepts a verified direct
            // managed input child; no child authorization is cached.
        }
        if (!this._alive)
            throw new Error('The Desk Switch companion is disabled');
        const sender = invocation.get_sender();
        const pid = await this._processId(sender);
        const child = await this._processIdentity(pid);
        const names = new Set([...this._icons.keys()].filter(icon =>
            icon._indicator.id === INDICATOR_ID && icon._indicator.busName)
            .map(icon => icon._indicator.busName));
        let parentName = null;
        for (const name of names) {
            if (await this._processId(name) === child.parent) {
                parentName = name;
                break;
            }
        }
        if (!parentName)
            throw new Error('Only a direct Desk Switch input child may observe the pointer');
        const parent = await this._processIdentity(child.parent);
        const uid = await this._userId(sender);
        if (await this._userId(parentName) !== uid ||
            !child.users.every(user => user === uid) || !parent.users.every(user => user === uid) ||
            await this._executable(pid) !== await this._executable(parent.pid))
            throw new Error('Input child executable or user does not match Desk Switch');
        const args = (await this._boundedText(`/proc/${pid}/cmdline`, 8192)).split('\0');
        if (args.at(-1) === '')
            args.pop();
        if (args.length !== 5 || !args[0] || args[1] !== 'run' || args[2] !== '--role' ||
            !['host', 'receiver'].includes(args[3]) || !args[4] || args[4].length > 128)
            throw new Error('The process is not a managed input worker');
        const environment = new Map();
        for (const entry of (await this._boundedText(`/proc/${pid}/environ`, 65536)).split('\0')) {
            const separator = entry.indexOf('=');
            if (separator < 0)
                continue;
            const key = entry.slice(0, separator);
            if (!['DESK_SWITCH_STATE', 'DESK_SWITCH_GROUP_INPUT', 'DESK_SWITCH_MANAGED'].includes(key))
                continue;
            if (environment.has(key))
                throw new Error('Input worker environment is ambiguous');
            environment.set(key, entry.slice(separator + 1));
        }
        const profile = environment.get('DESK_SWITCH_STATE');
        if (environment.get('DESK_SWITCH_GROUP_INPUT') !== '1' ||
            environment.get('DESK_SWITCH_MANAGED') !== '1' || !profile || profile.length > 4096 ||
            !GLib.path_is_absolute(profile))
            throw new Error('The input worker has no group profile');
        const marker = `${profile}/group-input`;
        const info = await this._fileInfo(marker, 'standard::type,unix::uid');
        if (info.get_file_type() !== Gio.FileType.REGULAR || info.get_attribute_uint32('unix::uid') !== uid ||
            (await this._boundedText(marker, 2)).trim() !== '1')
            throw new Error('The input worker group profile is unavailable');
        const finalChild = await this._processIdentity(pid);
        const finalParent = await this._processIdentity(parent.pid);
        if (!this._alive || await this._processId(sender) !== pid ||
            await this._processId(parentName) !== parent.pid ||
            finalChild.started !== child.started || finalChild.parent !== parent.pid ||
            finalParent.started !== parent.started ||
            !finalChild.users.every(user => user === uid) || !finalParent.users.every(user => user === uid))
            throw new Error('The input worker exited or changed identity');
        return pid;
    }

    async PointerStateAsync(_parameters, invocation) {
        try {
            await this._authorizePointer(invocation);
            if (!this._alive)
                throw new Error('The Desk Switch companion is disabled');
        } catch (error) {
            invocation.return_dbus_error(CLIPBOARD_NAME + '.PointerAccessDenied', error.message);
            return;
        }
        try {
            const [x, y] = global.get_pointer();
            if (![x, y].every(value => Number.isInteger(value) && value >= -2147483648 && value <= 2147483647))
                throw new Error('Pointer coordinates are unavailable');
            invocation.return_value(new GLib.Variant('(ii)', [x, y]));
        } catch (error) {
            this._clipboardError(invocation, error);
        }
    }

    _clipboardError(invocation, error) {
        invocation.return_dbus_error(CLIPBOARD_NAME + '.Unavailable', error.message);
    }

    async ReadAsync([knownSerial], invocation) {
        try {
            await this._authorizeCaller(invocation);
            if (!this._alive)
                throw new Error('The clipboard companion is disabled');
            const serial = this._clipboardSerial;
            if (this._clipboardRejected?.serial === serial)
                throw new Error(this._clipboardRejected.message);
            if (knownSerial === serial) {
                invocation.return_value(new GLib.Variant('(usay)', [serial, '', []]));
                return;
            }
            const reply = (mime, bytes) => {
                if (!this._alive || serial !== this._clipboardSerial) {
                    this._clipboardError(invocation, new Error('The clipboard changed; retry the read'));
                    return;
                }
                if (bytes.length > CLIPBOARD_LIMIT) {
                    const message = 'Clipboard content exceeds 8 MiB';
                    this._clipboardRejected = {serial, message};
                    this._clipboardError(invocation, new Error(message));
                    return;
                }
                invocation.return_value(new GLib.Variant('(usay)', [serial, mime, bytes]));
            };
            const type = St.ClipboardType.CLIPBOARD;
            const formats = this._clipboard.get_mimetypes(type);
            // Files are intentionally deferred; do not turn a file-manager copy
            // into an apparent successful transfer of its path text.
            if (formats.includes('x-special/gnome-copied-files')) {
                reply('', []);
            } else if (formats.includes('image/png')) {
                this._clipboard.get_content(type, 'image/png', (_clipboard, bytes) => {
                    if (bytes === null)
                        this._clipboardError(invocation, new Error('The clipboard changed; retry the read'));
                    else
                        reply('image/png', bytes.get_data());
                });
            } else if (formats.some(mime =>
                ['text/plain;charset=utf-8', 'UTF8_STRING', 'text/plain', 'STRING'].includes(mime))) {
                // Match St's supported MIME list exactly. Calling get_text with
                // an unsupported format may finish synchronously in GNOME.
                this._clipboard.get_text(type, (_clipboard, text) => {
                    if (text === null)
                        this._clipboardError(invocation, new Error('The clipboard changed; retry the read'));
                    else
                        reply('text/plain', new TextEncoder().encode(text));
                });
            } else {
                reply('', []);
            }
        } catch (error) {
            this._clipboardError(invocation, error);
        }
    }

    async WriteAsync([mime, bytes], invocation) {
        try {
            await this._authorizeCaller(invocation);
            if (!this._alive)
                throw new Error('The clipboard companion is disabled');
            if (!['text/plain', 'image/png'].includes(mime) || bytes.length > CLIPBOARD_LIMIT)
                throw new Error('Only text and PNG images up to 8 MiB are supported');
            if (mime === 'text/plain')
                this._clipboard.set_text(St.ClipboardType.CLIPBOARD,
                    new TextDecoder('utf-8', {fatal: true}).decode(Uint8Array.from(bytes)));
            else
                this._clipboard.set_content(St.ClipboardType.CLIPBOARD, mime,
                    new GLib.Bytes(Uint8Array.from(bytes)));
            this._clipboardSerial = (this._clipboardSerial + 1) >>> 0 || 1;
            this._clipboardRejected = null;
            invocation.return_value(new GLib.Variant('(u)', [this._clipboardSerial]));
        } catch (error) {
            this._clipboardError(invocation, error);
        }
    }

    _clearFocusRequest(pid) {
        const request = this._focusRequests.get(pid);
        if (!request)
            return;
        this._focusRequests.delete(pid);
        if (request.created)
            global.display.disconnect(request.created);
        if (request.timeout)
            GLib.Source.remove(request.timeout);
        for (const [window, signals] of request.windows) {
            for (const signal of signals)
                window.disconnect(signal);
        }
    }

    _requestFocus(pid, role) {
        const title = role === 'main' ? 'Desk Switch' :
            role === 'quick' ? 'Desk Switch quick controls' : null;
        if (!title)
            throw new Error('Unknown Desk Switch window role');
        this._clearFocusRequest(pid);
        const matches = window => window.get_pid() === pid && window.get_title() === title;
        const activate = window => {
            this._clearFocusRequest(pid);
            Main.activateWindow(window, global.get_current_time());
        };
        for (const actor of global.get_window_actors()) {
            if (matches(actor.meta_window)) {
                activate(actor.meta_window);
                return;
            }
        }
        // Wayland clients cannot raise an unfocused window themselves. Observe
        // only this process's newly created window for a bounded time; title
        // changes matter because window-created can precede set_title.
        const request = {created: 0, timeout: 0, windows: new Map()};
        this._focusRequests.set(pid, request);
        const watch = window => {
            if (!this._alive || window.get_pid() !== pid)
                return;
            if (matches(window)) {
                activate(window);
                return;
            }
            if (request.windows.has(window))
                return;
            const changed = window.connect('notify::title', () => {
                if (matches(window))
                    activate(window);
            });
            const unmanaged = window.connect('unmanaged', () => {
                const signals = request.windows.get(window);
                request.windows.delete(window);
                for (const signal of signals ?? [])
                    window.disconnect(signal);
            });
            request.windows.set(window, [changed, unmanaged]);
        };
        request.created = global.display.connect('window-created', (_display, window) => watch(window));
        request.timeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 2000, () => {
            request.timeout = 0;
            this._clearFocusRequest(pid);
            return GLib.SOURCE_REMOVE;
        });
        for (const actor of global.get_window_actors())
            watch(actor.meta_window);
    }

    async ActivateWindowAsync([role], invocation) {
        try {
            const pid = await this._authorizeCaller(invocation);
            if (!this._alive)
                throw new Error('The Desk Switch companion is disabled');
            this._requestFocus(pid, role);
            invocation.return_value(new GLib.Variant('()', []));
        } catch (error) {
            this._clipboardError(invocation, error);
        }
    }

    disable() {
        this._alive = false;
        for (const pid of [...this._focusRequests.keys()])
            this._clearFocusRequest(pid);
        this._cancellable.cancel();
        this._selection.disconnect(this._selectionSignal);
        this._selection = null;
        this._selectionSignal = 0;
        this._clipboardBridge.unexport();
        this._clipboardBridge = null;
        Gio.bus_unown_name(this._clipboardOwner);
        this._clipboardOwner = 0;
        this._allowedCallers.clear();
        global.stage.disconnect(this._stageCapture);
        this._stageCapture = 0;
        this._pressedIcon = null;
        if (this._scanSource)
            GLib.Source.remove(this._scanSource);
        this._scanSource = 0;
        for (const [box, signal] of this._boxes)
            box.disconnect(signal);
        this._boxes = [];
        for (const [icon, signal] of this._icons)
            icon.disconnect(signal);
        this._icons.clear();
        this._cancellable = null;
    }
}
