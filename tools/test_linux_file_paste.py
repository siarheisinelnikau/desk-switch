#!/usr/bin/env python3
"""Run the native file-paste gate on a disposable hosted Linux desktop.

The requested gate fails if FUSE, the private desktop, or an actual Nautilus
paste is unavailable. It never runs against an existing desktop or session.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time


TEST_NAME = (
    "group::clipboard_files::linux::vfs::tests::"
    "isolated_ci_native_deferred_file_paste"
)
CHILD_TIMEOUT = 80
STOP_TIMEOUT = 3
MAX_PROOF_BYTES = 64 * 1024
TOOLS = ("cargo", "xvfb-run", "Xvfb", "xauth", "dbus-run-session",
         "fusermount3", "nautilus", "xdotool", "openbox")


def require_hosted_runner():
    if (sys.platform != "linux" or os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"):
        raise RuntimeError("Native file paste requires a disposable GitHub-hosted Linux runner")
    runner_value = os.environ.get("RUNNER_TEMP")
    if not runner_value or not Path(runner_value).is_absolute():
        raise RuntimeError("RUNNER_TEMP must name existing disposable runner storage")
    runner = Path(runner_value).resolve(strict=True)
    if not runner.is_dir():
        raise RuntimeError("RUNNER_TEMP is not a directory")
    return runner


def preflight():
    missing = [tool for tool in TOOLS if shutil.which(tool) is None]
    if missing:
        raise RuntimeError("The requested native gate is missing: " + ", ".join(missing))
    try:
        fuse = Path("/dev/fuse").stat()
    except FileNotFoundError as error:
        raise RuntimeError("The requested native gate requires /dev/fuse") from error
    if not stat.S_ISCHR(fuse.st_mode) or not os.access("/dev/fuse", os.R_OK | os.W_OK):
        raise RuntimeError("The requested native gate needs readable and writable /dev/fuse")


def stop_group(process, timed_out):
    """Signal only the isolated process group created for this invocation."""
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    if timed_out:
        try:
            process.wait(timeout=STOP_TIMEOUT)
        except subprocess.TimeoutExpired:
            pass
    # The leader may have exited while a private desktop service is still in
    # its group. No process from the caller's desktop can join this new group.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    if process.poll() is None:
        process.wait(timeout=STOP_TIMEOUT)


def check_test_output(stdout):
    if TEST_NAME not in stdout or not re.search(
            r"test result: ok\. 1 passed; 0 failed; 0 ignored;", stdout):
        raise RuntimeError("Cargo did not report exactly one successful, unskipped native test")


def check_proof(value):
    expected = {
        "metadata_only_payload_demands": 0,
        "copy_complete_folder": True,
        "copy_destination_writable": True,
        "native_uri_reownership_preserves_current_offer": True,
        "native_move_complete_batch_receipts": 1,
        "partial_native_move_cancel_preserves_source_storage": True,
        "cancel_returns_native_error_instead_of_empty_eof": True,
    }
    if not isinstance(value, dict):
        raise RuntimeError("The native gate did not produce an object proof")
    for key, required in expected.items():
        if type(value.get(key)) is not type(required) or value[key] != required:
            raise RuntimeError(f"The native proof did not verify {key}")
    for key in ("provider_idle_cpu_ms_over_300ms", "provider_paused_cpu_ms_over_300ms"):
        if type(value.get(key)) is not int or not 0 <= value[key] < 80:
            raise RuntimeError(f"The native proof did not satisfy the settled CPU bound: {key}")
    scope = value.get("scope")
    source_scope = value.get("source_cleanup_scope")
    if not isinstance(scope, str) or not all(
            part in scope for part in ("Nautilus", "Ctrl+V", "X11", "Xvfb", "D-Bus", "FUSE")):
        raise RuntimeError("The native proof does not identify the tested desktop and paste path")
    if not isinstance(source_scope, str) or "does not delete originals" not in source_scope:
        raise RuntimeError("The native proof does not identify its source-cleanup boundary")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    # No output directory, fixture, native process, clipboard or mount exists
    # before this guard rejects an ordinary workstation or self-hosted runner.
    runner = require_hosted_runner()
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=True)
    report = {"test": TEST_NAME, "passed": False, "timed_out": False}
    runtime = None
    try:
        preflight()
        parent_display = os.environ.get("DISPLAY", "")
        parent_bus = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
        runtime = Path(tempfile.mkdtemp(prefix="desk-switch-native-file-paste-", dir=runner))
        runtime.chmod(0o700)
        runtime = runtime.resolve(strict=True)
        if runtime.parent != runner or runtime.stat().st_mode & 0o077:
            raise RuntimeError("The native gate runtime is not private runner storage")
        environment = dict(os.environ)
        for name in ("WAYLAND_DISPLAY", "DBUS_STARTER_ADDRESS", "DBUS_STARTER_BUS_TYPE",
                     "DBUS_SESSION_BUS_PID", "DBUS_SESSION_BUS_WINDOWID"):
            environment.pop(name, None)
        environment.update({
            "DESK_SWITCH_NATIVE_FILE_PASTE_TEST": "1",
            "DESK_SWITCH_NATIVE_PARENT_DISPLAY": parent_display,
            "DESK_SWITCH_NATIVE_PARENT_BUS": parent_bus,
            "DESK_SWITCH_NATIVE_TEST_RUNTIME": str(runtime),
            "RUNNER_TEMP": str(runner),
            "XDG_RUNTIME_DIR": str(runtime),
            "TMPDIR": str(runtime),
            "XDG_SESSION_TYPE": "x11",
            "GDK_BACKEND": "x11",
            "GSK_RENDERER": "cairo",
            "XDG_CONFIG_HOME": str(runtime / "config"),
            "XDG_CACHE_HOME": str(runtime / "cache"),
            "XDG_DATA_HOME": str(runtime / "data"),
            "NO_AT_BRIDGE": "1",
        })
        command = [
            "xvfb-run", "-a", "-s", "-screen 0 1920x1080x24",
            "dbus-run-session", "--",
            "cargo", "test", "--locked", TEST_NAME, "--",
            "--ignored", "--exact", "--nocapture", "--test-threads=1",
        ]
        start = time.monotonic()
        with (output / "stdout.log").open("wb") as stdout, (output / "stderr.log").open("wb") as stderr:
            process = subprocess.Popen(command, env=environment, stdin=subprocess.DEVNULL,
                                       stdout=stdout, stderr=stderr, start_new_session=True)
            try:
                process.wait(timeout=CHILD_TIMEOUT)
            except subprocess.TimeoutExpired:
                report["timed_out"] = True
            finally:
                stop_group(process, report["timed_out"])
        report["elapsed_seconds"] = round(time.monotonic() - start, 3)
        report["returncode"] = process.returncode
        proof_path = runtime / "native-file-paste-proof.json"
        if proof_path.is_file():
            with proof_path.open("rb") as proof:
                data = proof.read(MAX_PROOF_BYTES + 1)
            if len(data) > MAX_PROOF_BYTES:
                raise RuntimeError("The native gate proof exceeded its bounded size")
            (output / "native-file-paste-proof.json").write_bytes(data)
        else:
            data = None
        if report["timed_out"]:
            raise RuntimeError("The requested native gate exceeded its 80-second deadline")
        if process.returncode != 0:
            raise RuntimeError(f"The requested native gate failed with exit code {process.returncode}")
        check_test_output((output / "stdout.log").read_text(errors="replace"))
        if data is None:
            raise RuntimeError("The requested native gate did not produce its completion proof")
        check_proof(json.loads(data))
        report["passed"] = True
        print("Verified real Nautilus copy, move, cancellation and metadata-only clipboard publication.")
        return 0
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
        print(str(error), file=sys.stderr)
        for name in ("stdout.log", "stderr.log"):
            path = output / name
            if path.is_file():
                print(f"--- {name} ---\n" + path.read_text(errors="replace")[-32768:], file=sys.stderr)
        return 1
    finally:
        if runtime is not None:
            # Preserve this unique synthetic directory for hosted-runner
            # teardown. Recursive cleanup could traverse a failed FUSE mount.
            report["runtime"] = str(runtime)
            report["runtime_cleanup"] = "Disposable runner teardown; no recursive mount traversal"
        (output / "launcher-proof.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    raise SystemExit(main())
