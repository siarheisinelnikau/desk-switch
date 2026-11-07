#!/usr/bin/env python3
"""Exercise the password-free root helper without installing a package or changing policy."""
import hashlib
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
loader = importlib.machinery.SourceFileLoader('desk_switch_update_install', str(ROOT / 'packaging/linux/desk-switch-update-install'))
spec = importlib.util.spec_from_loader(loader.name, loader)
helper = importlib.util.module_from_spec(spec)
loader.exec_module(helper)

class UpdateInstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='desk-switch-helper-test-')
        self.root = Path(self.temporary.name)
        self.key = self.root / 'private.pem'
        subprocess.run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(self.key)], check=True, capture_output=True)
        self.public = subprocess.check_output(['openssl', 'pkey', '-in', str(self.key), '-pubout', '-outform', 'DER'])[-32:]
        self.package = self.root / 'original.deb'
        self.make_package()
        self.envelope = self.root / 'update.json'
        self.sign()

    def tearDown(self):
        self.temporary.cleanup()

    def make_package(self, **changes):
        stage = self.root / 'package'
        (stage / 'DEBIAN').mkdir(parents=True, exist_ok=True)
        values = dict(Package='desk-switch', Version='99.0.0', Architecture='amd64', Maintainer='Test <test@example.invalid>', Description='Synthetic fixture')
        values.update(changes)
        (stage / 'DEBIAN/control').write_text(''.join(f'{key}: {value}\n' for key, value in values.items()))
        subprocess.run(['dpkg-deb', '--root-owner-group', '--build', str(stage), str(self.package)], check=True, capture_output=True)

    def sign(self, **artifact_changes):
        artifact = {'url': 'https://github.com/siarheisinelnikau/desk-switch/releases/download/v99.0.0/desk-switch-v99.0.0-linux-x86-64.deb', 'size': self.package.stat().st_size, 'sha256': hashlib.sha256(self.package.read_bytes()).hexdigest()}
        artifact.update(artifact_changes)
        payload = json.dumps({'version': '99.0.0', 'targets': {'linux-x86_64-deb': artifact}}, separators=(',', ':'))
        message, signature = self.root / 'payload', self.root / 'signature'
        message.write_text(payload)
        subprocess.run(['openssl', 'pkeyutl', '-sign', '-rawin', '-inkey', str(self.key), '-in', str(message), '-out', str(signature)], check=True, capture_output=True)
        self.envelope.write_text(json.dumps({'payload': payload, 'signature': signature.read_bytes().hex()}))

    def prepare(self, key=None):
        work = self.root / 'verified'
        work.mkdir(exist_ok=True)
        return helper.prepare_verified(self.package, self.envelope, os.getuid(), work, self.public if key is None else key)

    def test_real_signature_and_package_are_copied_into_private_staging(self):
        version, copied = self.prepare()
        self.assertEqual(version, '99.0.0')
        self.assertNotEqual(copied, self.package)
        self.assertEqual(copied.read_bytes(), self.package.read_bytes())
        self.package.write_bytes(b'changed after verification')
        self.assertNotEqual(copied.read_bytes(), self.package.read_bytes())

    def test_modified_package_and_wrong_signing_key_are_rejected(self):
        self.package.write_bytes(self.package.read_bytes() + b'tamper')
        with self.assertRaises(ValueError): self.prepare()
        shutil.rmtree(self.root / 'verified')
        self.sign()
        with self.assertRaises(ValueError): self.prepare(bytes(32))

    def test_unsigned_manifest_edits_and_external_downloads_are_rejected(self):
        envelope = json.loads(self.envelope.read_text())
        envelope['payload'] = envelope['payload'].replace('99.0.0', '99.0.1')
        self.envelope.write_text(json.dumps(envelope))
        with self.assertRaises(ValueError): self.prepare()
        self.sign(url='https://example.invalid/desk-switch.deb')
        with self.assertRaises(ValueError): self.prepare()

    def test_signed_wrong_package_identity_is_still_rejected(self):
        for field, value in [('Package', 'another-package'), ('Architecture', 'arm64'), ('Version', '98.0.0')]:
            with self.subTest(field=field):
                self.make_package(**{field: value})
                self.sign()
                shutil.rmtree(self.root / 'verified', ignore_errors=True)
                with self.assertRaises(ValueError): self.prepare()

    def test_symlinks_wrong_owners_and_oversized_files_are_rejected(self):
        link = self.root / 'link'
        link.symlink_to(self.package)
        with self.assertRaises(OSError): helper.owned_file(link, os.getuid(), helper.MAX_PACKAGE)
        with self.assertRaises(ValueError): helper.owned_file(self.package, os.getuid() + 1, helper.MAX_PACKAGE)
        with self.assertRaises(ValueError): helper.owned_file(self.package, os.getuid(), 1)
        with self.assertRaises(ValueError): helper.owned_file(self.root, os.getuid(), helper.MAX_PACKAGE)

    def test_missing_authenticated_desktop_uid_never_starts_an_installer(self):
        with mock.patch.object(helper.os, 'geteuid', return_value=0), mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(helper.subprocess, 'run') as run:
            with self.assertRaises(ValueError): helper.install(self.package, self.envelope)
            run.assert_not_called()

    def test_reinstall_and_downgrade_are_rejected_before_apt(self):
        for current in ['99.0.0', '100.0.0']:
            with mock.patch.object(helper.subprocess, 'check_output', return_value=current):
                with self.assertRaises(ValueError): helper.require_newer('99.0.0')
        with mock.patch.object(helper.subprocess, 'check_output', return_value='98.0.0'):
            helper.require_newer('99.0.0')

    def test_policy_grants_only_fixed_helpers_to_active_users(self):
        for action, executable in [('input-access', 'input-access'), ('update', 'update-install')]:
            policy = ET.parse(ROOT / f'packaging/linux/com.deskswitch.{action}.policy').getroot()
            actions = policy.findall('action')
            self.assertEqual(len(actions), 1)
            defaults = actions[0].find('defaults')
            self.assertEqual([defaults.findtext(name) for name in ['allow_any', 'allow_inactive', 'allow_active']], ['no', 'no', 'yes'])
            self.assertEqual(actions[0].find("annotate[@key='org.freedesktop.policykit.exec.path']").text, f'/usr/lib/desk-switch/{executable}')

if __name__ == '__main__': unittest.main()
