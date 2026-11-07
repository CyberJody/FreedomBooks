"""Run with python3 crates/fb-harness/py/test_journal_aliases.py (no DB required)."""
import unittest
from types import SimpleNamespace
from journal_aliases import journal_names


class JournalAliases(unittest.TestCase):
    def test_order_and_distinct_ids(self):
        for pairs in [[('z', 1), ('é', 1), ('a', 1), ('other', 2)],
                      [('other', 2), ('a', 1), ('é', 1), ('z', 1)]]:
            runner = SimpleNamespace(refs={'journal': dict(pairs)})
            self.assertEqual(journal_names(runner), {1: 'a', 2: 'other'})

    def test_checkpoint_uses_only_current_aliases(self):
        runner = SimpleNamespace(refs={'journal': {'z': 1}})
        before = journal_names(runner)
        runner.refs['journal']['a'] = 1
        self.assertEqual(before, {1: 'z'})
        self.assertEqual(journal_names(runner), {1: 'a'})


if __name__ == '__main__':
    unittest.main()
