"""Supplemental verifier regressions: Decimal cents, posted-only and explicit scope.
Set FBV2_SAMPLE_TEST_DB to a disposable fbv2_l1_* database for the real PostgreSQL test.
The real test creates TEMP tables only; it never writes reference or seeded sample data.
"""
import json
import os
import runpy
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("check_sample_tb.py")


class SampleVerifierTest(unittest.TestCase):
    def run_verifier(self, psql):
        dump = {"companies": [{"ref": "sample", "name": "Synthetic sample"}],
                "accounts": [{"ref": "a", "code": "1000"}],
                "trial_balances": {"sample": {"2026-12-31": {"rows": [["a", 9999999999999999, 0]]}}}}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "dump.json"
            path.write_text(json.dumps(dump))
            with patch.object(sys, "argv", [str(SCRIPT), str(path)]), patch("subprocess.run", psql):
                with self.assertRaises(SystemExit) as result:
                    runpy.run_path(str(SCRIPT), run_name="__main__")
                self.assertEqual(result.exception.code, 0)

    def test_exact_cents_and_posted_company_date_scope(self):
        def psql(args, **kwargs):
            sql = args[-1]
            self.assertIn("CASE WHEN j.status = 'POSTED'", sql)
            self.assertIn("j.date <=", sql)
            self.assertIn("a.company_id =", sql)
            return subprocess.CompletedProcess(args, 0, "1000|99999999999999.99\n", "")
        self.run_verifier(psql)

    @unittest.skipUnless(os.environ.get("FBV2_SAMPLE_TEST_DB"), "set FBV2_SAMPLE_TEST_DB for actual PostgreSQL scope test")
    def test_actual_postgres_drafts_future_and_other_company_are_excluded(self):
        database = os.environ["FBV2_SAMPLE_TEST_DB"]
        self.assertTrue(database.startswith("fbv2_l1_"))
        actual_run = subprocess.run
        setup = """
        CREATE TEMP TABLE ledger_company(id integer, name text);
        CREATE TEMP TABLE ledger_account(id integer, company_id integer, code text);
        CREATE TEMP TABLE ledger_journal(id integer, company_id integer, status text, date date);
        CREATE TEMP TABLE ledger_journalline(account_id integer,journal_id integer,debit numeric(16,2),credit numeric(16,2));
        INSERT INTO ledger_company VALUES(1,'Synthetic sample'),(2,'Other company');
        INSERT INTO ledger_account VALUES(1,1,'1000'),(2,2,'1000');
        INSERT INTO ledger_journal VALUES(1,1,'POSTED','2026-01-01'),(2,1,'DRAFT','2026-01-02'),
            (3,1,'POSTED','2027-01-01'),(4,2,'POSTED','2026-01-01');
        INSERT INTO ledger_journalline VALUES(1,1,99999999999999.99,0),(1,2,10,0),(1,3,20,0),(2,4,30,0);
        """
        def psql(args, **kwargs):
            args = list(args)
            args[args.index("fbv2_l1_sample")] = database
            args[-1] = setup + args[-1]
            args.insert(1, "-q")
            return actual_run(args, **kwargs)
        self.run_verifier(psql)


if __name__ == "__main__":
    unittest.main()
