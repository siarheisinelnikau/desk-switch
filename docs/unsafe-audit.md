# Unsafe and FFI audit

Desk Switch talks to Windows, Linux and macOS through hand-written Win32, COM, Cloud Files API and FFI calls. Those paths carry the highest risk in the project: a wrong pointer, a missing handle close or a violated apartment rule is a memory-safety bug, not a compile error.

## Policy

Every `unsafe` block, `unsafe fn` and `unsafe impl` must be preceded by a `// SAFETY:` comment that states the invariant that makes the operation sound. The comment is required even when the call looks trivial.

A useful comment names the concrete facts, for example:

```rust
// SAFETY: the handle is owned by this guard, no other copy exists, and the
// guard closes it exactly once when it is dropped.
unsafe { CloseHandle(self.0) }
```

```rust
// SAFETY: the destination buffer holds exactly `count` elements and the call
// writes at most `count` of them.
unsafe { GetRawPointerBuffer(self.data, &mut self.ptr, &mut self.len) }
```

```rust
// SAFETY: the interface pointer was created in the STA apartment of this
// thread and is only used from the same thread before it is released.
unsafe { SomeInterface::from_raw(ptr) }
```

A comment that only says "this is an FFI call", "safe" or "the pointer is valid" without naming the invariant is treated as a defect in review. If the invariant cannot be stated, the block needs a design change, not a louder comment.

Prefer a narrow safe wrapper over a raw call. A wrapper that owns the handle, validates the buffer length and exposes a typed result removes the need for `unsafe` at every call site.

## Enforcement

`clippy::undocumented_unsafe_blocks` is enabled at `deny` level in `Cargo.toml`, so a new unsafe block without a `// SAFETY:` comment fails `cargo clippy` and therefore CI and the release build. The audit reached zero undocumented blocks, which is why the lint can be enforced instead of merely reported.

`tools/audit_unsafe.py` reports the remaining undocumented blocks per file:

```sh
python tools/audit_unsafe.py
```

The report is advisory and always exits successfully; the supply-chain workflow publishes it so the count cannot grow unnoticed.

## Priority order

The audit starts where the risk is highest:

1. `src/group/clipboard_files/windows.rs` and `windows/cloud.rs` — Cloud Files placeholders, CFAPI callbacks and COM objects.
2. `src/group/file_transfer/source_trash/windows.rs` — Recycle Bin work, ACL manipulation and identity checks.
3. `src/group/clipboard_files/windows/cloud_api.rs` and `windows/destination.rs` — Shell and destination discovery.
4. `src/platform/win_cursor.rs`, `win_input.rs`, `win_receive.rs` — raw input, cursor and injection paths.
5. `src/platform/win_monitor.rs` — DDC/CI and display enumeration.
6. The remaining modules: audio mixers, autostart, cable extraction, macOS geometry.

## Rules that apply everywhere

- A handle is owned by exactly one value and released once, normally in `Drop`.
- A pointer is never stored beyond the lifetime of the buffer it points into.
- COM objects are created and used in the same apartment, and the apartment is entered before the object is created.
- Raw buffers are sized from the API that reports the length, and the length is checked against the maximum the caller accepts.
- No `unsafe` block in a hot loop that can allocate or block without an explicit bound.
- Never widen a pointer or cast an integer into one without an intermediate and a documented bound.

## Related checks

The native proofs in `tools/test_windows_file_paste.py` and `tools/test_linux_file_paste.py` exercise the ACL, recycle, clipboard and paste paths on a disposable runner. They are the behavioural counterpart of this documentation audit.
