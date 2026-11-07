// Exercise the shipped extension's event handlers and clipboard API with fake
// Shell objects. No real pointer events, Shell session or clipboard is touched.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import vm from 'node:vm';

class Signals {
    constructor() { this.handlers = new Map(); this.next = 1; }
    connect(name, callback) { const id = this.next++; this.handlers.set(id, {name, callback}); return id; }
    disconnect(id) { assert(this.handlers.delete(id)); }
    emit(name, ...args) { for (const handler of this.handlers.values()) if (handler.name === name) handler.callback(this, ...args); }
}
const stage = new Signals();
let target = null;
stage.get_actor_at_pos = () => target;
const selection = new Signals();
const display = new Signals();
display.get_selection = () => selection;
const windows = [], focused = [];
const formats = [];
let text = '', png = new Uint8Array(), reads = 0, writes = 0;
const clipboard = {
    get_mimetypes() { return [...formats]; },
    get_text(_type, callback) { reads++; callback(this, text); },
    get_content(_type, _mime, callback) { reads++; callback(this, png === null ? null : {get_data: () => png}); },
    set_text(_type, value) { writes++; text = value; formats.splice(0, formats.length, 'text/plain;charset=utf-8'); selection.emit('owner-changed', 1); },
    set_content(_type, mime, bytes) { writes++; png = bytes.data; formats.splice(0, formats.length, mime); selection.emit('owner-changed', 1); },
};
const idle = new Map(), timeouts = new Map();
let sourceId = 0, exports = 0, owned = 0;
const GLib = {
    PRIORITY_DEFAULT_IDLE: 0, PRIORITY_DEFAULT: 0, SOURCE_REMOVE: false,
    idle_add(_priority, callback) { const id = ++sourceId; idle.set(id, callback); return id; },
    timeout_add(_priority, duration, callback) { assert.equal(duration, 2000); const id = ++sourceId; timeouts.set(id, callback); return id; },
    Source: {remove(id) { assert(idle.delete(id) || timeouts.delete(id)); }},
    Variant: class { constructor(signature, data) { this.signature = signature; this.data = data; } deep_unpack() { return this.data; } },
    Bytes: class { constructor(data) { this.data = data; } },
};
const Gio = {
    Cancellable: class { cancel() { this.cancelled = true; } },
    IOErrorEnum: {CANCELLED: 1}, BusType: {SESSION: 0}, BusNameOwnerFlags: {NONE: 0}, DBusCallFlags: {NONE: 0},
    DBusExportedObject: {wrapJSObject() { return {export() { exports++; }, unexport() { exports--; }}; }},
    DBus: {session: {
        call(_name, _path, _interface, method, arguments_, _reply, _flags, _timeout, _cancel, callback) {
            assert.equal(method, 'GetConnectionUnixProcessID');
            const name = arguments_.data[0];
            queueMicrotask(() => callback(this, {pid: [':1.4', ':1.5'].includes(name) ? 4200 : 9000}));
        },
        call_finish(result) { return {deep_unpack: () => [result.pid]}; },
    }},
    bus_own_name() { return ++owned; }, bus_unown_name() { owned--; },
};
const calls = [];
function icon(id, busName, x) {
    const actor = new Signals();
    Object.assign(actor, {
        visible: true, _indicator: {id, busName, _proxy: {}, open: async (...args) => calls.push(args)},
        menu: {closes: 0, close() { this.closes++; }},
        get_transformed_extents: () => ({get_x: () => x, get_y: () => 0, get_width: () => 24, get_height: () => 24}),
        contains: other => other?.parent === actor,
    });
    return actor;
}
const own = icon('desk-switch', ':1.4', 100), other = icon('another-app', ':1.9', 130);
const boxes = [new Signals(), new Signals(), new Signals()];
const Main = {panel: {_leftBox: boxes[0], _centerBox: boxes[1], _rightBox: boxes[2], statusArea: {own, other}}, activateWindow: (window, timestamp) => focused.push({window, timestamp})};
const Clutter = {EventType: {BUTTON_PRESS: 1, BUTTON_RELEASE: 2}, BUTTON_PRIMARY: 1, EVENT_PROPAGATE: false, EVENT_STOP: true, PickMode: {REACTIVE: 0}};
const sandbox = {Clutter, Gio, GLib, Meta: {SelectionType: {SELECTION_CLIPBOARD: 1}}, St: {Clipboard: {get_default: () => clipboard}, ClipboardType: {CLIPBOARD: 1}}, Main, Extension: class {}, global: {stage, display, get_window_actors: () => windows.map(meta_window => ({meta_window})), get_current_time: () => 1234}, console, TextEncoder, TextDecoder};
const source = (await readFile(new URL('../packaging/linux/gnome-extension/extension.js', import.meta.url), 'utf8'))
    .replace(/^import .*;\r?\n/gm, '').replace('export default class DeskSwitchTray', 'globalThis.Subject = class DeskSwitchTray');
vm.runInNewContext(source, sandbox, {filename: 'Desk Switch GNOME extension.js'});
const extension = new sandbox.Subject();
extension.enable();
assert.equal(idle.size, 1);
for (const [id, callback] of idle) { idle.delete(id); callback(); }
assert.equal(extension._icons.size, 2);
const event = (type, button = 1, x = 110) => ({type: () => type, get_button: () => button, get_coords: () => [x, 12], get_time: () => 1234});
target = own;
assert.equal(extension._handleEvent(event(1, 3)), Clutter.EVENT_PROPAGATE);
assert.equal(extension._handleEvent(event(2, 3)), Clutter.EVENT_PROPAGATE);
assert.equal(own.menu.closes, 0);
assert.equal(extension._handleEvent(event(1)), Clutter.EVENT_STOP);
assert.equal(extension._handleEvent(event(2)), Clutter.EVENT_STOP);
await Promise.resolve();
assert.deepEqual(calls, [[110, 12, 1234]]);
assert.equal(own.menu.closes, 1);
target = other;
assert.equal(extension._handleEvent(event(1, 1, 140)), Clutter.EVENT_PROPAGATE);
assert.equal(extension._handleEvent(event(2, 1, 140)), Clutter.EVENT_PROPAGATE);
target = own;
extension._handleEvent(event(1)); target = null; extension._handleEvent(event(2));
assert.equal(calls.length, 1, 'a cancelled press must not activate');
target = {parent: own};
extension._handleEvent(event(1)); extension._handleEvent(event(2));
await Promise.resolve();
assert.equal(calls.length, 2, 'a child actor belongs to its indicator');
const fallback = [];
own._indicator.open = undefined;
own._indicator._proxy.ActivateAsync = async (...args) => fallback.push(args);
await extension._activate(own._indicator, 110.4, 12.2, 1234);
assert.equal(fallback.length, 1, 'older hosts use their standard SNI activation');
assert.deepEqual(fallback[0].slice(0, 2), [110, 12]);

class Invocation {
    constructor(sender = ':1.5') { this.sender = sender; }
    get_sender() { return this.sender; }
    return_value(value) { assert(!this.result && !this.error); this.result = value; }
    return_dbus_error(name, message) { assert(!this.result && !this.error); this.error = {name, message}; }
}
formats.push('text/plain;charset=utf-8'); text = 'Hello — привет\n';
const first = new Invocation(); await extension.ReadAsync([0], first);
assert.equal(first.result.signature, '(usay)'); assert.equal(first.result.data[1], 'text/plain');
assert.equal(new TextDecoder().decode(Uint8Array.from(first.result.data[2])), text);
const repeat = new Invocation(); await extension.ReadAsync([first.result.data[0]], repeat);
assert.equal(repeat.result.data[1], ''); assert.equal(reads, 1, 'unchanged clipboard must not be read again');
const denied = new Invocation(':1.99'); await extension.ReadAsync([0], denied);
assert(denied.error); assert.equal(reads, 1);
const write = new Invocation(); await extension.WriteAsync(['image/png', [137, 80, 78, 71]], write);
assert(write.result); assert.equal(writes, 1);
const image = new Invocation(); await extension.ReadAsync([0], image);
assert.equal(image.result.data[1], 'image/png'); assert.deepEqual(Array.from(image.result.data[2]), [137, 80, 78, 71]);
formats.splice(0, formats.length, 'x-special/gnome-copied-files', 'text/plain');
selection.emit('owner-changed', 1);
const files = new Invocation(); await extension.ReadAsync([0], files);
assert.equal(files.result.data[1], ''); assert.equal(reads, 2, 'files must remain deferred');
const invalid = new Invocation(); await extension.WriteAsync(['text/uri-list', []], invalid);
assert(invalid.error); assert.equal(writes, 1);
formats.splice(0, formats.length, 'text/plain-unrelated');
selection.emit('owner-changed', 1);
const unsupported = new Invocation(); await extension.ReadAsync([0], unsupported);
assert.equal(unsupported.result.data[1], '');
assert.equal(reads, 2, 'St.get_text must not be invoked for unsupported MIME types');
formats.splice(0, formats.length, 'text/plain'); text = null;
selection.emit('owner-changed', 1);
const failedRead = new Invocation(); await extension.ReadAsync([0], failedRead);
assert(failedRead.error, 'an unavailable text selection must not erase the other computers clipboard');
text = '';
const empty = new Invocation(); await extension.ReadAsync([0], empty);
assert.equal(empty.result.data[1], 'text/plain'); assert.equal(empty.result.data[2].length, 0);
formats.splice(0, formats.length, 'image/png'); png = null;
selection.emit('owner-changed', 1);
const failedImage = new Invocation(); await extension.ReadAsync([0], failedImage);
assert(failedImage.error, 'an unavailable PNG selection must remain retryable');
text = 'x'.repeat(8 * 1024 * 1024 + 1);
formats.splice(0, formats.length, 'text/plain'); selection.emit('owner-changed', 1);
const oversized = new Invocation(); await extension.ReadAsync([0], oversized); assert(oversized.error);
const afterOversized = reads;
const repeatedOversized = new Invocation(); await extension.ReadAsync([0], repeatedOversized); assert(repeatedOversized.error);
assert.equal(reads, afterOversized, 'unchanged oversized content must not be reread on each poll');
function window(pid, title) {
    const result = new Signals();
    Object.assign(result, {pid, title, get_pid: () => result.pid, get_title: () => result.title});
    return result;
}
const unrelated = window(9000, 'Desk Switch'), main = window(4200, 'Desk Switch');
windows.push(unrelated, main);
const activateMain = new Invocation(); await extension.ActivateWindowAsync(['main'], activateMain);
assert(activateMain.result); assert.equal(focused.length, 1); assert.equal(focused[0].window, main);
assert.equal(focused[0].timestamp, 1234); assert.equal(timeouts.size, 0);
const deniedFocus = new Invocation(':1.99'); await extension.ActivateWindowAsync(['main'], deniedFocus);
assert(deniedFocus.error); assert.equal(focused.length, 1, 'another process cannot raise Desk Switch windows');
const arbitrary = new Invocation(); await extension.ActivateWindowAsync(['any title'], arbitrary);
assert(arbitrary.error); assert.equal(focused.length, 1, 'only fixed Desk Switch window roles are supported');
const activateQuick = new Invocation(); await extension.ActivateWindowAsync(['quick'], activateQuick);
assert(activateQuick.result); assert.equal(timeouts.size, 1); assert.equal(display.handlers.size, 1);
display.emit('window-created', unrelated); assert.equal(unrelated.handlers.size, 0);
const quick = window(4200, null); windows.push(quick); display.emit('window-created', quick);
assert.equal(quick.handlers.size, 2, 'a newly created window may receive its title later');
quick.title = 'Desk Switch quick controls'; quick.emit('notify::title');
assert.equal(focused.length, 2); assert.equal(focused[1].window, quick);
assert.equal(timeouts.size, 0); assert.equal(display.handlers.size, 0); assert.equal(quick.handlers.size, 0);
windows.splice(windows.indexOf(quick), 1);
const timeoutFocus = new Invocation(); await extension.ActivateWindowAsync(['quick'], timeoutFocus);
for (const [id, callback] of timeouts) { timeouts.delete(id); callback(); }
assert.equal(extension._focusRequests.size, 0); assert.equal(display.handlers.size, 0);
const pendingFocus = new Invocation(); await extension.ActivateWindowAsync(['quick'], pendingFocus);
assert.equal(timeouts.size, 1);
extension._queueScan(); assert.equal(idle.size, 1);
const late = new Invocation();
const inFlight = extension.WriteAsync(['text/plain', [88]], late);
extension.disable();
await inFlight;
assert(late.error); assert.equal(writes, 1, 'disabling cancels an authenticated write before clipboard access');
assert.equal(idle.size, 0); assert.equal(stage.handlers.size, 0); assert.equal(selection.handlers.size, 0);
assert.equal(timeouts.size, 0); assert.equal(display.handlers.size, 0);
assert.equal(main.handlers.size, 0); assert.equal(unrelated.handlers.size, 0);
assert(boxes.every(box => box.handlers.size === 0)); assert.equal(own.handlers.size, 0); assert.equal(other.handlers.size, 0);
assert.equal(exports, 0); assert.equal(owned, 0);
console.log('PASS: shipped GNOME handlers keep right-click native, activate one left click, ignore other apps, authorize clipboard callers by process, transfer text/image bytes, raise only own fixed-title windows with bounded one-shot observers and clean up signals. Mock Shell only; physical Ubuntu routing/focus remains unverified.');
