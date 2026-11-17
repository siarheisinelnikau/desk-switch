#!/usr/bin/env python3
"""Run Android's real JVM transport against the desktop with a synthetic sink."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-binary', type=Path)
    parser.add_argument('--out', type=Path, default=ROOT / 'target/android-interop')
    parser.add_argument('--offline', action='store_true', help='Use the local Cargo and Gradle caches')
    args = parser.parse_args()
    offline = ['--offline'] if args.offline else []
    if args.test_binary is None:
        result = subprocess.run(['cargo', 'test', '--locked', '--features', 'android-remote',
            '--jobs', '2', '--no-run', '--message-format=json', *offline], cwd=ROOT,
            stdout=subprocess.PIPE, text=True, check=True)
        artifacts = (json.loads(line) for line in result.stdout.splitlines() if line.startswith('{'))
        args.test_binary = next(Path(item['executable']) for item in artifacts
            if item.get('reason') == 'compiler-artifact' and item.get('executable')
            and item['target']['name'] == 'desk-switch' and item['profile']['test'])
    output = args.out.resolve() / uuid.uuid4().hex
    output.mkdir(parents=True)
    (output / '.isolated-mobile-proof').touch()
    environment = dict(os.environ, DESK_SWITCH_MOBILE_PROOF=str(output))
    if args.offline:
        environment['CARGO_NET_OFFLINE'] = 'true'
    with (output / 'desktop.log').open('wb') as log:
        process = subprocess.Popen([str(args.test_binary.resolve()),
            'mobile::tests::external_android_client_controls_a_fake_sink', '--ignored', '--exact', '--nocapture'],
            cwd=ROOT, env=environment, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 20
            while not (output / 'ready.json').exists():
                assert process.poll() is None, (output / 'desktop.log').read_text(errors='replace')
                assert time.monotonic() < deadline, 'The isolated desktop fixture did not become ready'
                time.sleep(0.05)
            wrapper = ROOT / 'android' / ('gradlew.bat' if os.name == 'nt' else 'gradlew')
            with (output / 'android-jvm.log').open('wb') as android_log:
                result = subprocess.run([str(wrapper), ':core:test', '--tests', '*DesktopInteropTest',
                    '--no-daemon', '--max-workers=2', *offline], cwd=wrapper.parent,
                    env=environment, stdout=android_log, stderr=subprocess.STDOUT, timeout=90)
            assert result.returncode == 0, (output / 'android-jvm.log').read_text(errors='replace')
            assert process.wait(timeout=15) == 0, (output / 'desktop.log').read_text(errors='replace')
            proof = json.loads((output / 'complete.json').read_text())
            assert proof['native_devices'] is False and proof['released'] > 0 and proof['text_commits'] == 1
            assert json.loads((output / 'paired.json').read_text())['code'] == (output / 'phone-code.txt').read_text()
            print('PASS: Android Noise-XX/KK, matching pairing codes, original input frames, ordered Unicode text, encrypted synthetic cursor preview, and disconnect release without native devices.')
            print(f'Proof: {output}')
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)


if __name__ == '__main__':
    main()
