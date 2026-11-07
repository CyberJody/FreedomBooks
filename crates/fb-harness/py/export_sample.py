"""Seed the Python app's sample company (ledger.sample_company, every document service) on a fresh
fbv2_l1_* database, then export its LEDGER as a harness scenario: the chart, and every journal in creation
order as a manual entry (save_draft, then post_journal if posted) or a reverse_journal of the exported
original (reason from its journal.reversed audit event). Document links are dropped (layer 2); the books
(accounts, lines, dates, memos, adjusting flags, reversals) are what layer 1 must reproduce.

    FBV2_DB=fbv2_l1_sample . crates/fb-harness/py/env.sh
    $FB_PY crates/fb-harness/py/export_sample.py crates/fb-harness/scenarios/sample-company.json
"""
import json
import os
import sys

sys.path.insert(0, os.environ["FB_REFERENCE"])
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "config.settings")
import django  # noqa: E402

django.setup()
from django.conf import settings  # noqa: E402
from django.contrib.auth import get_user_model  # noqa: E402

from ledger.models import Account, AuditEvent, Journal  # noqa: E402
from ledger.sample_company import seed_sample_company  # noqa: E402

assert settings.DATABASES["default"]["NAME"].startswith("fbv2_l1_"), "refusing to run outside an fbv2_l1_* database"
AS_OF = "2026-10-29"


def main():
    out_path = sys.argv[1]
    owner = get_user_model().objects.create_user(username="owner")
    company = seed_sample_company(as_of=AS_OF, username=owner.username)
    steps = [{"op": "create_company", "ref": "sample", "name": company.name, "currency": company.currency}]
    account_ref = {}
    for account in Account.objects.filter(company=company).order_by("pk"):
        ref = f"a{account.code}"
        account_ref[account.pk] = ref
        steps.append({"op": "save_account", "ref": ref, "company": "sample", "code": account.code, "name": account.name,
                      "detail_type": account.detail_type, "is_contra": account.is_contra, "description": account.description,
                      "active": account.active, "postable": account.postable})
    journal_ref = {}
    reasons = {}
    for event in AuditEvent.objects.filter(company=company, action="journal.reversed"):
        reasons[int(event.target.split(":")[1])] = event.detail["reason"]
    counts = {"journals": 0, "reversals": 0, "drafts": 0, "lines": 0}
    for journal in Journal.objects.filter(company=company).order_by("pk"):
        ref = f"j{journal.pk}"
        journal_ref[journal.pk] = ref
        if journal.reversal_of_id:
            steps.append({"op": "reverse_journal", "ref": ref, "company": "sample", "journal": journal_ref[journal.reversal_of_id],
                          "date": str(journal.date), "reason": reasons[journal.reversal_of_id]})
            counts["reversals"] += 1
            continue
        lines = []
        for line in journal.lines.all():
            item = {"account": account_ref[line.account_id]}
            item["debit" if line.debit else "credit"] = str(line.debit or line.credit)
            if line.description:
                item["description"] = line.description
            if line.name:
                item["name"] = line.name
            lines.append(item)
        counts["lines"] += len(lines)
        step = {"op": "save_draft", "ref": ref, "company": "sample", "date": str(journal.date), "memo": journal.memo, "lines": lines}
        if journal.entry_number:
            step["entry_number"] = journal.entry_number
        elif journal.source_id:
            # Document journals carry no entry number; as manual entries they need one that cannot collide
            # with the sample's own manual entry numbers (auto-numbering would take 1, 2, 3, ...).
            step["entry_number"] = f"DOC-{journal.pk}"
        if journal.adjusting:
            step["adjusting"] = True
        steps.append(step)
        counts["journals"] += 1
        if journal.status == "POSTED":
            steps.append({"op": "post_journal", "company": "sample", "journal": ref})
        else:
            counts["drafts"] += 1
    scenario = {"name": "sample-company", "source": f"ledger.sample_company.seed_sample_company(as_of={AS_OF!r}) on {settings.DATABASES['default']['NAME']}",
                "users": [{"username": "owner"}, {"username": "stranger"}], "steps": steps,
                "as_of": ["2026-06-30", "2026-09-30", AS_OF, "2026-12-31"]}
    with open(out_path, "w") as handle:
        json.dump(scenario, handle, indent=1, ensure_ascii=False)
    print(out_path, len(steps), "steps", counts)


if __name__ == "__main__":
    main()
