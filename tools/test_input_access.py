#!/usr/bin/env python3
"""Exercise input-authorization boundaries without opening devices or changing ACLs."""
import importlib.machinery
import importlib.util
import os
from pathlib import Path
import stat
import types
import tempfile
from unittest.mock import Mock
import unittest
from unittest.mock import patch

path = Path(__file__).resolve().parents[1] / 'packaging/linux/desk-switch-input-access'
loader = importlib.machinery.SourceFileLoader('input_access', str(path))
spec = importlib.util.spec_from_loader(loader.name, loader)
helper = importlib.util.module_from_spec(spec)
loader.exec_module(helper)


class InputAccessTests(unittest.TestCase):
    def test_arbitrary_files_and_non_evdev_nodes_are_rejected(self):
        for value in ['/etc/shadow', '/dev/sda', '/dev/input/event1/elsewhere', '/tmp/event2']:
            with patch.object(Path, 'resolve', return_value=Path(value)):
                with self.assertRaises(ValueError): helper.input_path(value)
        for mode, uid, rdev in [(stat.S_IFREG, 0, 0), (stat.S_IFCHR, 1000, os.makedev(13, 66)),
                                (stat.S_IFCHR, 0, os.makedev(8, 1)), (stat.S_IFCHR, 0, os.makedev(13, 1))]:
            with self.assertRaises(ValueError):
                helper.supported(Path('/dev/input/event2'), types.SimpleNamespace(st_mode=mode, st_uid=uid, st_rdev=rdev))

    def test_physical_relative_input_is_required(self):
        metadata = types.SimpleNamespace(st_mode=stat.S_IFCHR, st_uid=0, st_rdev=os.makedev(13, 66))
        for bus, axes, valid in [('0003', 1 << 32, True), ('0006', 0, False), ('0003', 1, False)]:
            values = {'bustype': bus, 'key': hex(1 << 272)[2:], 'rel': '3', 'abs': hex(axes)[2:]}
            # Convert the sparse keyboard bitmap into the sysfs word layout.
            values['key'] = '10000 0 0 0 0'
            with patch.object(Path, 'read_text', lambda p: values[p.name]):
                if valid: helper.supported(Path('/dev/input/event2'), metadata)
                else:
                    with self.assertRaises(ValueError): helper.supported(Path('/dev/input/event2'), metadata)

    def test_entire_request_is_validated_before_granting_access(self):
        with patch.object(os, 'geteuid', return_value=0), patch.dict(os.environ, {'PKEXEC_UID':'1000'}), \
             patch.object(helper, 'input_path', side_effect=[Path('/dev/input/event2'), Path('/dev/input/event3')]), \
             patch.object(os, 'open', side_effect=[71,72]), patch.object(os, 'fstat'), \
             patch.object(helper, 'inspected', side_effect=[{}, ValueError('wrong device')]), \
             patch.object(os, 'close') as close, patch.object(helper.subprocess, 'run') as run:
            with self.assertRaises(ValueError): helper.grant(['first','second'])
            run.assert_not_called()
            self.assertEqual(close.call_count, 2)

    def test_only_the_authenticated_desktop_uid_and_pinned_inode_are_used(self):
        with patch.object(os, 'geteuid', return_value=0), patch.dict(os.environ, {'PKEXEC_UID':'1001'}), \
             patch.object(helper, 'input_path', return_value=Path('/dev/input/event2')), \
             patch.object(os, 'open', return_value=71), patch.object(os, 'fstat'), \
             patch.object(helper, 'inspected', return_value={}), patch.object(os, 'close'), \
             patch.object(helper.subprocess, 'run') as run:
            helper.grant(['/dev/input/by-id/a-keyboard'], store=Mock())
            args, options = run.call_args
            self.assertEqual(args[0], ['/usr/bin/setfacl','--modify','u:1001:r','/proc/self/fd/71'])
            self.assertEqual(options['pass_fds'], (71,))
        for uid in ['', '0', '4294967296', '1000; id', '１０００']:
            with patch.object(os, 'geteuid', return_value=0), patch.dict(os.environ, {'PKEXEC_UID':uid}):
                with self.assertRaises(ValueError): helper.grant(['/dev/input/event2'])


class PersistentInputTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='desk-switch-access-test-')
        self.parent = Path(self.temporary.name) / 'desk-switch'
        self.directory = self.parent / 'input-access'
        self.store = helper.Store(self.directory, owner=os.getuid())
        self.uid = os.getuid() or 1000
        self.account = types.SimpleNamespace(pw_name='fixture-user')
        self.user_patch = patch.object(helper.pwd, 'getpwuid', return_value=self.account)
        self.user_patch.start()
        self.device = dict(bustype='0003', vendor='046d', product='c52b', version='0111',
                           name='Fixture USB Keyboard', uniq='', serial='serial-A', keyboard=True, mouse=False)

    def tearDown(self):
        self.user_patch.stop()
        self.temporary.cleanup()

    def test_grant_survives_a_new_store_and_event_number_without_matching_other_hardware(self):
        self.store.save(self.uid, [self.device])
        restarted = helper.Store(self.directory, owner=os.getuid())
        self.assertEqual(restarted.matching_users(dict(self.device)), [self.uid])
        for key, value in [('serial','serial-B'), ('product','ffff'), ('name','Another Keyboard'), ('mouse',True)]:
            self.assertEqual(restarted.matching_users(dict(self.device, **{key:value})), [])
        self.assertEqual((self.directory / f'{self.uid}.json').stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.directory.stat().st_mode & 0o777, 0o700)

    def test_noserial_models_are_scoped_by_bus_ids_name_and_capabilities(self):
        model = dict(self.device, serial='')
        self.store.save(self.uid, [model])
        self.assertEqual(self.store.matching_users(model), [self.uid])
        self.assertEqual(self.store.matching_users(dict(model, bustype='0005')), [])
        self.assertEqual(self.store.matching_users(dict(model, name='Fixture Mouse', keyboard=False, mouse=True)), [])

    def test_an_added_device_keeps_existing_grants_and_duplicates_are_coalesced(self):
        self.store.save(self.uid, [self.device])
        mouse = dict(self.device, name='Fixture Mouse', keyboard=False, mouse=True)
        self.store.save(self.uid, [self.device, mouse])
        self.assertEqual(self.store.read(self.uid)['devices'], [self.device, mouse])

    def test_udev_restores_only_preapproved_users_using_the_current_pinned_inode(self):
        self.store.save(self.uid, [self.device])
        with patch.object(os, 'geteuid', return_value=0), \
             patch.object(helper, 'input_path', return_value=Path('/dev/input/event19')), \
             patch.object(os, 'open', return_value=71), patch.object(os, 'close'), \
             patch.object(helper, 'inspected', return_value=dict(self.device)), \
             patch.object(helper, 'apply') as apply:
            # Do not let a forged pkexec environment choose the restored uid.
            with patch.dict(os.environ, {'PKEXEC_UID':'9999'}):
                store = Mock();store.matching_users.return_value=[self.uid]
                helper.restore('event19', store=store)
            apply.assert_called_once_with(self.uid, 71)
            store.matching_users.assert_called_once_with(self.device)
        with patch.object(os, 'geteuid', return_value=0), patch.object(os, 'open') as opened:
            for value in ['../event2','/dev/input/event2','event2/../event3','event2;id']:
                with self.assertRaises(ValueError): helper.restore(value, store=self.store)
            opened.assert_not_called()

    def test_user_writable_or_symlinked_state_cannot_authorize_input(self):
        self.store.save(self.uid, [self.device])
        profile = self.directory / f'{self.uid}.json'
        profile.chmod(0o666)
        self.assertEqual(self.store.matching_users(self.device), [])
        profile.chmod(0o600)
        outside = Path(self.temporary.name) / 'outside.json'
        profile.rename(outside)
        profile.symlink_to(outside)
        self.assertEqual(self.store.matching_users(self.device), [])
        profile.unlink();outside.rename(profile)
        self.directory.chmod(0o777)
        with self.assertRaises(ValueError): self.store.matching_users(self.device)
        self.directory.chmod(0o700)
        self.assertEqual(self.store.matching_users(self.device), [self.uid])

    def test_deleted_or_reused_desktop_accounts_do_not_inherit_permission(self):
        self.store.save(self.uid, [self.device])
        with patch.object(helper.pwd, 'getpwuid', return_value=types.SimpleNamespace(pw_name='another-user')):
            self.assertEqual(self.store.matching_users(self.device), [])
        with patch.object(helper.pwd, 'getpwuid', side_effect=KeyError('deleted')):
            self.assertEqual(self.store.matching_users(self.device), [])

    def test_virtual_or_malformed_saved_identities_are_rejected(self):
        for device in [dict(self.device, bustype='0006'), dict(self.device, vendor='*'),
                       dict(self.device, name='injected\nrule'), dict(self.device, mouse='true'),
                       dict(self.device, keyboard=False)]:
            with self.assertRaises(ValueError): self.store.save(self.uid, [device])


if __name__ == '__main__': unittest.main()
