#!/usr/bin/env python3
"""Validate the existing updater key and upload it through authenticated GitHub CLI."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = 'siarheisinelnikau/desk-switch'
SECRET = 'UPDATE_SIGNING_KEY_PEM'


def gh(*arguments, data=None):
    result = subprocess.run(['gh', *arguments, '--repo', REPOSITORY], input=data, capture_output=True)
    if result.returncode:
        raise SystemExit('GitHub CLI failed; secret values suppressed. Check gh auth status and environment access.')
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='Validate locally without network or uploading')
    parser.add_argument('--replace', action='store_true', help='Allow updating an existing environment secret')
    args = parser.parse_args()
    key = ROOT / '.release-private/update-signing-key.pem'
    result = subprocess.run(['openssl', 'pkey', '-in', str(key), '-pubout', '-outform', 'DER'], capture_output=True)
    public = bytes.fromhex((ROOT / 'release/update-public-key.hex').read_text().strip())
    if result.returncode or result.stdout != bytes.fromhex('302a300506032b6570032100') + public:
        raise SystemExit('The private key does not match the Ed25519 key pinned in installed clients.')
    print('Local private key matches the pinned Ed25519 public key.')
    if args.check:
        return
    if not shutil.which('gh'):
        raise SystemExit('Install GitHub CLI and run gh auth login, then run this command again.')
    existing = {item['name'] for item in json.loads(gh('secret', 'list', '--env', 'release', '--json', 'name'))}
    if SECRET in existing and not args.replace:
        raise SystemExit('The signing secret already exists. Use --replace only after checking this local identity.')
    gh('secret', 'set', SECRET, '--env', 'release', data=key.read_bytes())
    saved = {item['name'] for item in json.loads(gh('secret', 'list', '--env', 'release', '--json', 'name'))}
    if SECRET not in saved:
        raise SystemExit('The environment is still missing the signing secret.')
    print('Uploaded and verified ' + SECRET + ' in ' + REPOSITORY + ' / release; the value was not printed.')


if __name__ == '__main__':
    main()
