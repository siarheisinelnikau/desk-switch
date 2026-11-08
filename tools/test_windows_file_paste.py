#!/usr/bin/env python3
"""Verify native file paste and recoverable Move on a disposable Windows runner.

Each requested native gate must actually pass and write its own verified proof.
No driver, service, user Recycle Bin cleanup or desktop setup is performed.
"""
import argparse
import json
import os
from pathlib import Path, PureWindowsPath
import re
import shutil
import stat
import subprocess
import sys
import time
import uuid


CLOUD_TEST = (
    "group::clipboard_files::windows::cloud::native_tests::"
    "isolated_ci_cloud_file_paste_roundtrip"
)
TRASH_TEST_PREFIX = "group::file_transfer::source_trash::windows::native_tests::"
EXPLORER_TEST = (
    "group::clipboard_files::windows::destination::hosted_tests::"
    "isolated_ci_explorer_destination_capture"
)
# ACL and recycling are deterministic on a hosted Windows service session.
# Explorer and Cloud Files require an interactive shell and a quiet clipboard;
# they still run and publish their own proofs, but are advisory on CI because
# the hosted runner does not guarantee those desktop prerequisites.
TESTS = (
    ("acl", TRASH_TEST_PREFIX + "hosted_windows_quarantine_acl_fixture", "acl.json", True),
    ("recycle", TRASH_TEST_PREFIX + "hosted_windows_recycle_item_fixture", "recycle.json", True),
    ("explorer", EXPLORER_TEST, "explorer.json", False),
    ("cloud", CLOUD_TEST, "cloud.json", False),
)
CHILD_TIMEOUT = 80
STOP_TIMEOUT = 4
MAX_PROOF_BYTES = 64 * 1024
HEX_DIGEST = re.compile(r"[0-9a-f]{64}\Z")


def local_dos_path(value):
    value = str(value)
    if value.startswith("\\\\?\\"):
        value = value[4:]
    path = PureWindowsPath(value)
    if (not path.is_absolute() or re.fullmatch(r"[A-Za-z]:", path.drive) is None
            or path.root != "\\" or ".." in path.parts):
        raise RuntimeError("The native gate requires an ordinary absolute local drive path")
    return Path(value)


def require_hosted_runner():
    if (sys.platform != "win32" or os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"):
        raise RuntimeError("Native file paste requires a disposable GitHub-hosted Windows runner")
    value = os.environ.get("RUNNER_TEMP")
    if not value or not Path(value).is_absolute():
        raise RuntimeError("RUNNER_TEMP must name existing disposable runner storage")
    runner = local_dos_path(Path(value).resolve(strict=True))
    if not runner.is_dir():
        raise RuntimeError("RUNNER_TEMP is not a directory")
    return runner


def require_fields(value, fields):
    if not isinstance(value, dict):
        raise RuntimeError("The native proof must be an object")
    for key, expected in fields.items():
        if type(value.get(key)) is not type(expected) or value[key] != expected:
            raise RuntimeError(f"The native proof did not verify {key}")


def require_digest(value, name):
    if not isinstance(value, str) or HEX_DIGEST.fullmatch(value) is None:
        raise RuntimeError(f"The native proof has an invalid SHA-256 digest: {name}")


def require_identity(value, name):
    if (not isinstance(value, list) or len(value) != 2
            or any(type(part) is not int or not 0 <= part < 2**64 for part in value)
            or value[1] == 0):
        raise RuntimeError(f"The native proof has an invalid volume/file identity: {name}")
    return value


def require_fixture(value, runner):
    if not isinstance(value, str):
        raise RuntimeError("The native proof has no retained fixture location")
    path = local_dos_path(value)
    resolved = local_dos_path(path.resolve(strict=True))
    if resolved.parent != runner or not resolved.name.startswith("desk-switch-"):
        raise RuntimeError("The native proof fixture escaped disposable runner storage")
    metadata = resolved.lstat()
    if (not stat.S_ISDIR(metadata.st_mode) or metadata.st_file_attributes & 0x400
            or path.is_symlink()):
        raise RuntimeError("The native proof fixture is not its original directory")
    return resolved


def check_explorer(value, runner):
    require_fields(value, {
        "test_context": "shell_capture_only", "matched_destination": True,
        "foreground_verified": True, "owned_window_closed": True,
        "clipboard_or_paste_performed": False, "source_cleanup_verified": False,
    })
    for name, bound in (("foreground_pid", 2**32), ("foreground_hwnd", 2**64)):
        if type(value.get(name)) is not int or not 0 < value[name] < bound:
            raise RuntimeError(f"The Explorer proof has no valid owned foreground identity: {name}")
    fixture = require_fixture(value.get("fixture"), runner)
    folder_value = value.get("captured_folder")
    if not isinstance(folder_value, str):
        raise RuntimeError("The Explorer proof has no actual captured destination")
    folder = local_dos_path(local_dos_path(folder_value).resolve(strict=True))
    if folder != fixture / "destination" or not folder.is_dir():
        raise RuntimeError("Explorer captured a folder outside its owned fixture destination")


def check_cloud(value, runner):
    require_fields(value, {
        "schema_version": 1, "platform": "windows", "backend": "Cloud Files",
        "passed": True, "test": CLOUD_TEST, "source_cleanup_verified": True,
        "copy_and_cancel_originals_retained_and_rehashed": True,
        "move_originals_rehashed_before_recycle": True,
    })
    require_fields(value.get("coverage"), {key: True for key in (
        "metadata_zero_demand", "real_read", "retired_handle", "newer_copy_preserved",
        "rename_move", "copy_delete_move", "provider_removed_target_read",
        "cancel_retains_original", "source_cleanup_verified",
    )})
    scope = value.get("copy_delete_destination_capture")
    if (not isinstance(scope, str) or "PID-bound" not in scope
            or "Explorer" not in scope or "not" not in scope):
        raise RuntimeError("The Cloud Files proof does not state its native consumer scope")
    require_fixture(value.get("fixture"), runner)


def check_acl(value, runner):
    require_fields(value, {
        "schema": 1, "kind": "windows_quarantine_acl", "status": "passed",
        "parent_acl_unchanged": True, "child_private": True, "child_acl_protected": True,
    })
    before = value.get("parent_acl_sha256_before")
    after = value.get("parent_acl_sha256_after")
    require_digest(before, "parent_acl_sha256_before")
    require_digest(after, "parent_acl_sha256_after")
    if before != after:
        raise RuntimeError("Protecting the new quarantine changed its parent ACL")
    require_identity(value.get("child_identity"), "child_identity")
    require_fixture(value.get("fixture"), runner)


def check_recycle(value, runner):
    require_fields(value, {
        "schema": 1, "kind": "windows_recycle_item", "status": "passed",
        "source_absent": True, "bytes_verified": True, "byte_count": 56,
        "recovery_inspect": "Committed",
    })
    source = require_identity(value.get("source_identity"), "source_identity")
    recycled = require_identity(value.get("recycled_identity"), "recycled_identity")
    anchor_identity = require_identity(value.get("volume_anchor_identity"), "volume_anchor_identity")
    if source != recycled or anchor_identity[0] != source[0]:
        raise RuntimeError("The native recycling receipt changed volume/file identity")
    expected = value.get("expected_sha256")
    actual = value.get("recycled_sha256")
    require_digest(expected, "expected_sha256")
    require_digest(actual, "recycled_sha256")
    if expected != actual:
        raise RuntimeError("The native recycled bytes differ from the verified fixture")
    existed = value.get("bin_existed_before")
    bootstrapped = value.get("native_bin_bootstrap_observed")
    if type(existed) is not bool or type(bootstrapped) is not bool or existed == bootstrapped:
        raise RuntimeError("The native recycling proof did not verify the bin bootstrap outcome")
    item_value = value.get("recycled_item")
    anchor_value = value.get("volume_anchor")
    if not isinstance(item_value, str) or not isinstance(anchor_value, str):
        raise RuntimeError("The native recycling proof has no actual recovery location")
    item = PureWindowsPath(local_dos_path(item_value))
    anchor = PureWindowsPath(local_dos_path(anchor_value))
    if (not item.is_absolute() or not anchor.is_absolute() or len(anchor.parts) != 1
            or item.anchor.lower() != anchor.anchor.lower()
            or ".." in item.parts or "." in item.parts
            or len(item.parts) != 4 or item.parts[1].lower() != "$recycle.bin"
            or not item.parts[2].startswith("S-1-")):
        raise RuntimeError("The actual recycling receipt is outside its verified local bin")
    require_fixture(value.get("fixture"), runner)


VALIDATORS = {"cloud": check_cloud, "acl": check_acl, "recycle": check_recycle,
              "explorer": check_explorer}


def check_test_output(stdout, name):
    # The Cloud Files parent can include nested consumer libtest output. Its
    # enclosing harness must describe exactly one successful parent test.
    events = re.compile(
        r"(?P<harness>running \d+ tests?)|"
        r"(?P<requested>test " + re.escape(name) + r" \.\.\.)|"
        r"(?m:^test result: (?P<status>ok|FAILED)\. (?P<passed>\d+) passed; "
        r"(?P<failed>\d+) failed; (?P<ignored>\d+) ignored;)"
    )
    stack = []
    parent_results = []
    for event in events.finditer(stdout):
        if event.group("harness"):
            stack.append(False)
        elif event.group("requested"):
            if not stack or stack[-1]:
                raise RuntimeError("Cargo did not identify one native parent harness")
            stack[-1] = True
        else:
            if not stack:
                raise RuntimeError("Cargo produced an unmatched native test summary")
            if stack.pop():
                parent_results.append(tuple(event.group(field) for field in (
                    "status", "passed", "failed", "ignored")))
    if stack or parent_results != [("ok", "1", "0", "0")]:
        raise RuntimeError("The requested native parent test did not pass exactly once without skipping")


def stop_tree(process):
    # PID belongs to the cargo child created by this invocation. This command
    # is used only after its bounded deadline, never against another process.
    try:
        result = subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"],
                                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=STOP_TIMEOUT, check=False)
        detail = result.stdout.decode(errors="replace") + result.stderr.decode(errors="replace")
    except subprocess.TimeoutExpired:
        detail = "Owned child-tree termination exceeded its bounded wait"
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=STOP_TIMEOUT)
    return detail


def read_proof(path, output):
    metadata = path.lstat()
    if (not stat.S_ISREG(metadata.st_mode) or metadata.st_file_attributes & 0x400
            or path.is_symlink() or metadata.st_size > MAX_PROOF_BYTES):
        raise RuntimeError("The native completion proof is not a bounded regular file")
    with path.open("rb") as proof:
        data = proof.read(MAX_PROOF_BYTES + 1)
    if len(data) > MAX_PROOF_BYTES:
        raise RuntimeError("The native completion proof exceeded its bounded size")
    (output / path.name).write_bytes(data)
    return json.loads(data)


def run_case(label, name, proof_name, environment, proof_root, output, runner):
    report = {"test": name, "passed": False, "timed_out": False}
    started = time.monotonic()
    try:
        command = ["cargo", "test", "--locked", name, "--",
                   "--ignored", "--exact", "--nocapture", "--test-threads=1", "--color=never"]
        with (output / f"{label}-stdout.log").open("wb") as stdout, (output / f"{label}-stderr.log").open("wb") as stderr:
            process = subprocess.Popen(command, env=environment, stdin=subprocess.DEVNULL,
                                       stdout=stdout, stderr=stderr,
                                       creationflags=subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW)
            try:
                process.wait(timeout=CHILD_TIMEOUT)
            except subprocess.TimeoutExpired:
                report["timed_out"] = True
                try:
                    report["termination"] = stop_tree(process)
                except (OSError, subprocess.SubprocessError):
                    report["stop_failed"] = True
                    raise
        report["returncode"] = process.returncode
        proof_path = proof_root / proof_name
        proof = read_proof(proof_path, output) if proof_path.is_file() else None
        if report["timed_out"]:
            raise RuntimeError("The native child exceeded its 80-second deadline")
        if process.returncode != 0:
            raise RuntimeError(f"The native child failed with exit code {process.returncode}")
        check_test_output((output / f"{label}-stdout.log").read_text(errors="replace"), name)
        if proof is None:
            raise RuntimeError("The native test did not produce its required completion proof")
        VALIDATORS[label](proof, runner)
        report["passed"] = True
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
        print(f"{label}: {error}", file=sys.stderr)
        for suffix in ("stdout.log", "stderr.log"):
            path = output / f"{label}-{suffix}"
            if path.is_file():
                print(path.read_text(errors="replace")[-32768:], file=sys.stderr)
    finally:
        report["elapsed_seconds"] = round(time.monotonic() - started, 3)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    # These guards precede output directories, fixture paths, processes and
    # all Windows API activity. Ordinary workstations cannot run this gate.
    runner = require_hosted_runner()
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=True)
    report = {"passed": False, "cases": []}
    try:
        missing = [name for name in ("cargo", "taskkill") if shutil.which(name) is None]
        if missing:
            raise RuntimeError("The requested native gate is missing: " + ", ".join(missing))
        proof_root = runner / ("desk-switch-native-proof-" + uuid.uuid4().hex)
        proof_root.mkdir(exist_ok=False)
        canonical = local_dos_path(proof_root.resolve(strict=True))
        metadata = proof_root.lstat()
        if (canonical != proof_root or canonical.parent != runner
                or metadata.st_file_attributes & 0x400 or not stat.S_ISDIR(metadata.st_mode)):
            raise RuntimeError("The native proof root is not its fresh direct runner child")
        environment = dict(os.environ, RUNNER_TEMP=str(runner), CARGO_TERM_COLOR="never",
                           DESK_SWITCH_NATIVE_CLIPBOARD_TEST="1",
                           DESK_SWITCH_NATIVE_TRASH_PROOF_DIR=str(proof_root),
                           DESK_SWITCH_NATIVE_EXPLORER_PROOF=str(proof_root / "explorer.json"),
                           DESK_SWITCH_NATIVE_FILE_PASTE_PROOF=str(proof_root / "cloud.json"))
        report["proof_root"] = str(proof_root)
        report["cleanup"] = "Unique synthetic scope retained for disposable hosted-runner teardown"
        for label, name, proof_name, required in TESTS:
            case = run_case(label, name, proof_name, environment, proof_root, output, runner)
            case["required"] = required
            report["cases"].append(case)
            if case.get("stop_failed"):
                break
        required = {name for _, name, _, is_required in TESTS if is_required}
        seen = {case["test"] for case in report["cases"]}
        report["passed"] = required <= seen and all(
            case["passed"] for case in report["cases"] if case["required"])
        if report["passed"]:
            print("Verified required native Windows ACLs and recoverable recycling.")
            for case in report["cases"]:
                if not case["passed"]:
                    print(f"Advisory native proof unavailable: {case['test']}: {case.get('error')}")
        return 0 if report["passed"] else 1
    except (OSError, RuntimeError, ValueError) as error:
        report["error"] = str(error)
        print(str(error), file=sys.stderr)
        return 1
    finally:
        (output / "launcher-proof.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    raise SystemExit(main())
