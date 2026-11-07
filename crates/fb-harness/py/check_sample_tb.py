"""Cross-check sample-company Rust balances against the ORIGINAL seeded sample's posted ledger.
Usage: python3 check_sample_tb.py target/harness/sample-company.rust.json [company-name]
Only the selected synthetic company, posted entries and selected as-of date are included.
"""
import json
import subprocess
import sys
from decimal import Decimal


def cents(value):
    scaled = Decimal(value) * 100
    if scaled != scaled.to_integral_value():
        raise ValueError(f"Not whole cents: {value}")
    return int(scaled)


def main():
    with open(sys.argv[1]) as handle:
        dump = json.load(handle)
    company = next(c for c in dump["companies"] if c["ref"] == "sample")
    company_name = sys.argv[2] if len(sys.argv) > 2 else company["name"]
    dates = dump["trial_balances"]["sample"]
    as_of = "2099-12-31" if "2099-12-31" in dates else "2026-12-31"
    refs = {a["ref"]: a["code"] for a in dump["accounts"] if a.get("company", "sample") == "sample"}
    rust = {refs[ref]: debit - credit for ref, debit, credit in dates[as_of]["rows"]}
    literal = lambda value: "'" + value.replace("'", "''") + "'"
    sql = (
        "SELECT a.code, coalesce(sum(CASE WHEN j.status = 'POSTED' "
        f"AND j.date <= {literal(as_of)} AND j.company_id = a.company_id THEN l.debit - l.credit ELSE 0 END), 0) "
        "FROM ledger_account a LEFT JOIN ledger_journalline l ON l.account_id = a.id "
        "LEFT JOIN ledger_journal j ON j.id = l.journal_id "
        f"WHERE a.company_id = (SELECT id FROM ledger_company WHERE name = {literal(company_name)}) "
        "GROUP BY a.code ORDER BY a.code"
    )
    out = subprocess.run(["psql", "-X", "-At", "-F", "|", "fbv2_l1_sample", "-c", sql], capture_output=True, text=True, check=True).stdout
    python = {code: cents(net) for code, net in (row.split("|") for row in out.strip().splitlines())}
    diff = {code: (python.get(code), rust.get(code)) for code in set(python) | set(rust) if python.get(code) != rust.get(code)}
    print("accounts", len(python), "differences", diff or "none")
    return 1 if diff else 0


if __name__ == "__main__":
    sys.exit(main())
