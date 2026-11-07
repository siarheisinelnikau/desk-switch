#!/usr/bin/env python3
"""Exercise input-authorization boundaries without opening devices or changing ACLs."""
import importlib.machinery
import importlib.util
import os
from pathlib import Path
import stat
import types
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
             patch.object(helper, 'supported', side_effect=[None, ValueError('wrong device')]), \
             patch.object(os, 'close') as close, patch.object(helper.subprocess, 'run') as run:
            with self.assertRaises(ValueError): helper.grant(['first','second'])
            run.assert_not_called()
            self.assertEqual(close.call_count, 2)

    def test_only_the_authenticated_desktop_uid_and_pinned_inode_are_used(self):
        with patch.object(os, 'geteuid', return_value=0), patch.dict(os.environ, {'PKEXEC_UID':'1001'}), \
             patch.object(helper, 'input_path', return_value=Path('/dev/input/event2')), \
             patch.object(os, 'open', return_value=71), patch.object(os, 'fstat'), \
             patch.object(helper, 'supported'), patch.object(os, 'close'), \
             patch.object(helper.subprocess, 'run') as run:
            helper.grant(['/dev/input/by-id/a-keyboard'])
            args, options = run.call_args
            self.assertEqual(args[0], ['/usr/bin/setfacl','--modify','u:1001:r','/proc/self/fd/71'])
            self.assertEqual(options['pass_fds'], (71,))
        for uid in ['', '0', '4294967296', '1000; id', '１０００']:
            with patch.object(os, 'geteuid', return_value=0), patch.dict(os.environ, {'PKEXEC_UID':uid}):
                with self.assertRaises(ValueError): helper.grant(['/dev/input/event2'])


if __name__ == '__main__': unittest.main()
