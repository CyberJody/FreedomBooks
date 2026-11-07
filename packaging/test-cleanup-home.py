#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('cleanup_home', Path(__file__).with_name('cleanup-home.py'))
cleanup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cleanup)


class Cleanup(unittest.TestCase):
    def test_mount_is_not_traversed_or_deleted(self):
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp) / 'home'
            portal = home / 'runtime/doc'
            portal.mkdir(parents=True)
            (portal / 'document').write_text('must not touch')
            (home / 'ordinary').write_text('remove')
            self.assertFalse(cleanup.remove_unmounted(home, {portal}))
            self.assertEqual((portal / 'document').read_text(), 'must not touch')
            self.assertFalse((home / 'ordinary').exists())
            self.assertTrue(cleanup.remove_unmounted(home, set()))
            self.assertFalse(home.exists())


if __name__ == '__main__':
    unittest.main(verbosity=2)
