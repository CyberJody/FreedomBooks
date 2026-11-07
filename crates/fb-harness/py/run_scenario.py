"""Answer-key runner: apply a FreedomBooks harness scenario through the frozen Python services and dump
the books as normalized JSON (natural keys, money in integer cents).

    FBV2_DB=fbv2_l1_harness . crates/fb-harness/py/env.sh
    $FB_PY crates/fb-harness/py/run_scenario.py scenario.json out.json

The database named by FBV2_DB must be a fresh, migrated copy (the harness driver makes it from the
template). Only existing services are called (ledger.services, ledger.reports); the frozen app is not
modified. Users are created with Django's create_user and memberships with company.members.add, the
same calls the app's tests and sample-company seeder use.
"""
import json
import copy
import os
import sys
from decimal import Decimal
from journal_aliases import journal_names

sys.path.insert(0, os.environ["FB_REFERENCE"])
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "config.settings")
import django  # noqa: E402

django.setup()
from django.conf import settings  # noqa: E402
from django.contrib.auth import get_user_model  # noqa: E402
from django.contrib.auth.models import AnonymousUser  # noqa: E402
from django.core.exceptions import ObjectDoesNotExist, PermissionDenied, ValidationError  # noqa: E402
from django.db import IntegrityError, connection  # noqa: E402

from backups_adapter import apply_backups, identity as backup_identity, cleanup as cleanup_backups
from checkpoints import reports as checkpoint_reports
from attachments_adapter import apply_attachments, dump_attachments, normalize_attachment_audit
from attachments.sniff import Refused
from ledger import reports, services  # noqa: E402
from l2a import apply_l2a, dump_l2a, audit_maps, normalize_audit
from prepayments_adapter import apply_prepayments, dump_prepayments, extend_pp_audit_ids, normalize_pp_audit
from sales_adapter import apply_sales, dump_sales, normalize_l2, sales_audit_maps, normalize_sales_audit
from credits_adapter import apply_credits, dump_credits, credits_l2, extend_credits_audit_ids, normalize_credits_audit
from checks_adapter import apply_checks, dump_checks, extend_checks_audit_ids, normalize_checks_audit
from assistant_adapter import apply_assistant, dump_assistant, extend_assistant_audit, normalize_assistant_audit
from payables_adapter import apply_payables, dump_payables, payables_l2, extend_payables_audit_ids, normalize_payables_audit
from reports_adapter import dump_reports
from banking_adapter import apply_banking, dump_banking, banking_l2, extend_banking_audit, normalize_banking_audit
from bankfeeds_adapter import apply_bankfeeds, dump_bankfeeds, extend_bankfeeds_audit, normalize_bankfeeds_audit, finalize as bankfeeds_finalize, manifest as bankfeeds_manifest
from ledger.models import Account, AuditEvent, Company, Journal  # noqa: E402

assert settings.DATABASES["default"]["NAME"].startswith(("fbv2_l4f_", "fbv2_l4b_", "fbv2_l4t_", "fbv2_l4a_", "fbv2_l4r_", "fbv2_l1_", "fbv2_l2a_", "fbv2_l3s_", "fbv2_l3p_", "fbv2_l3c_", "fbv2_core_", "fbv2_l3d_", "fbv2_int", "fbv2_l3b_", "fbv2_l3r_")), "refusing to run outside an approved fbv2 harness database"
MISSING_PK = 999999999


class MissingRef(Exception):
    """A step names a company / account / journal whose creating step was refused."""


def ref_of(table, ref):
    if ref not in table:
        raise MissingRef(f"unknown ref {ref}")
    return table[ref]


def cents(value):
    return int((Decimal(value) * 100).to_integral_value())


class Runner:
    def __init__(self, scenario):
        self.scenario = scenario
        self.users = {}
        self.refs = {"company": {}, "account": {}, "journal": {}}
        self.company_actor = {}
        self.outcomes = []
        self.checkpoints = []
        self.l2_refs = {}
        self.l2_results = {}

    def actor(self, name):
        if name == "<anonymous>":
            return AnonymousUser()
        return self.users[name]

    def company(self, ref):
        return Company.objects.get(pk=ref_of(self.refs["company"], ref))

    def account_pk(self, ref):
        return self.refs["account"].get(ref, MISSING_PK)

    def lines(self, lines):
        out = []
        for line in lines:
            line = dict(line)
            if "account" in line and isinstance(line["account"], str):
                line["account"] = self.account_pk(line["account"])
            out.append(line)
        return out

    def run(self):
        User = get_user_model()
        for user in self.scenario.get("users", []):
            self.users[user["username"]] = User.objects.create_user(username=user["username"], is_staff=user.get("is_staff", False))
        for step in self.scenario["steps"]:
            self.run_step(step)

    def run_step(self, step):
        index = len(self.outcomes)
        outcome = {"step": index, "op": step["op"]}
        try:
            self.apply(step)
            outcome["ok"] = True
            if step['op'] in ('assistant_parse','assistant_initial','assistant_queue'): outcome['result_ref']=step['ref']
        except Refused as exc:
            outcome.update(ok=False, kind="Refused", message=str(exc))
        except ValidationError as exc:
            outcome.update(ok=False, kind="ValidationError", message="; ".join(exc.messages))
        except PermissionDenied as exc:
            outcome.update(ok=False, kind="PermissionDenied", message=str(exc))
        except ObjectDoesNotExist as exc:
            outcome.update(ok=False, kind="DoesNotExist", message=str(exc))
        except IntegrityError as exc:
            outcome.update(ok=False, kind="IntegrityError", message=str(exc).splitlines()[0])
        except MissingRef as exc:
            outcome.update(ok=False, kind="MissingRef", message=str(exc))
        if (outcome["ok"] == step.get("expect_error", False)
                or ("expect_kind" in step and outcome.get("kind") != step["expect_kind"])
                or ("expect_message" in step and not step["op"].startswith("bankfeeds_") and outcome.get("message") != step["expect_message"])):
            outcome["unexpected"] = True
        # Driver evidence is independent of attachment query/result generation.
        outcome['audit_counts'] = {
            ref: AuditEvent.objects.filter(company_id=pk).count()
            for ref, pk in self.refs['company'].items()
        }
        if outcome['ok'] and step['op'] in ('reminder_build', 'reminder_count', 'attachment_download'):
            outcome['query'] = {key: step.get(key) for key in ('ref', 'company', 'today', 'days')}
            outcome['query']['attachment'] = self.attachment_refs[step['attachment']] if step['op'] == 'attachment_download' else None
        # A refusal's pinned reason travels with the outcome; the comparer holds each side to it
        # (after v2's redaction of this side's bank-feed text, which this runner cannot apply).
        for pin in ("expect_kind", "expect_message"):
            if pin in step:
                outcome[pin] = step[pin]
        manifest = bankfeeds_manifest(step)
        if manifest is not None:
            outcome["manifest"] = manifest
        evidence = getattr(self, 'bf', {}).pop('evidence', None)
        if evidence is not None and outcome["ok"]:
            outcome["evidence"] = evidence

        self.outcomes.append(outcome)
        if self.scenario.get('capture_steps'):
            self.checkpoints.append(dict(step=index,dump=copy.deepcopy(self.dump_current()),financial=checkpoint_reports(self)))
        return outcome

    def apply(self, step):
        op = step["op"]
        actor = self.actor(step.get("actor", self.scenario["users"][0]["username"]))
        if apply_backups(self,step,actor):
            return
        if apply_bankfeeds(self,step,actor) or apply_assistant(self,step,actor) or apply_attachments(self,step,actor) or apply_banking(self,step,actor) or apply_credits(self,step,actor) or apply_prepayments(self,step,actor) or apply_checks(self,step,actor) or apply_sales(self,step,actor) or apply_payables(self,step,actor):
            return
        if apply_l2a(self,step,actor):
            return
        if op == "create_company":
            company = services.create_company(actor, name=step["name"], currency=step.get("currency", "USD"),
                                              open_first_run=step.get("open_first_run", False))
            self.refs["company"][step["ref"]] = company.pk
            self.company_actor.setdefault(step["ref"], actor)
        elif op == "add_member":
            self.company(step["company"]).members.add(self.users[step["user"]])
        elif op == "edit_company":
            services.edit_company(actor, self.company(step["company"]), name=step["name"], currency=step["currency"])
        elif op == "save_account":
            account = Account.objects.get(pk=ref_of(self.refs["account"], step["account"])) if step.get("account") else None
            saved = services.save_account(actor, self.company(step["company"]), code=step["code"], name=step["name"],
                                          kind=step.get("kind"), detail_type=step.get("detail_type"),
                                          is_contra=step.get("is_contra"), description=step.get("description"),
                                          active=step.get("active", True), postable=step.get("postable", True), account=account)
            if step.get("ref") and not step.get("account"):
                self.refs["account"][step["ref"]] = saved.pk
        elif op == "save_draft":
            journal_id = ref_of(self.refs["journal"], step["journal"]) if step.get("journal") else None
            journal = services.save_draft(actor, self.company(step["company"]), date=step["date"], memo=step["memo"],
                                          lines=self.lines(step["lines"]), journal_id=journal_id,
                                          entry_number=step.get("entry_number"), adjusting=step.get("adjusting"),
                                          request_key=step.get("request_key"), expected_revision=step.get("expected_revision"))
            if step.get("ref") and not step.get("journal"):
                self.refs["journal"].setdefault(step["ref"], journal.pk)
        elif op == "post_journal":
            services.post_journal(actor, self.company(step["company"]), ref_of(self.refs["journal"], step["journal"]),
                                  expected_revision=step.get("expected_revision"))
        elif op == "reverse_journal":
            reversal = services.reverse_journal(actor, self.company(step["company"]), ref_of(self.refs["journal"], step["journal"]),
                                                date=step["date"], reason=step["reason"])
            self.refs["journal"][step["ref"]] = reversal.pk
        else:
            raise SystemExit(f"unknown op {op}")

    def dump(self):
        current = self.dump_current()
        return dict(checkpoints=self.checkpoints, final=current) if self.scenario.get('capture_steps') else current

    def dump_current(self):
        company_ref = {pk: ref for ref, pk in self.refs["company"].items()}
        account_ref = {pk: ref for ref, pk in self.refs["account"].items()}
        journal_ref = journal_names(self)
        companies = Company.objects.filter(pk__in=company_ref).order_by("pk")
        outcomes = copy.deepcopy(self.outcomes)
        attachment_refs = {}
        for name, pk in sorted(getattr(self, 'attachment_refs', {}).items()):
            attachment_refs.setdefault(pk, name)
        for outcome in outcomes:
            query = outcome.get('query')
            if query and query['attachment'] is not None:
                query['attachment'] = attachment_refs[query['attachment']]
        out = {"outcomes": outcomes, "companies": [], "accounts": [], "journals": [], "display": {},
               "registers": {}, "trial_balances": {}, "audit": {}}
        l2_ids = sales_audit_maps(self, audit_maps(self))
        extend_payables_audit_ids(self, l2_ids)
        extend_checks_audit_ids(self, l2_ids)
        extend_pp_audit_ids(self, l2_ids)
        extend_credits_audit_ids(self, l2_ids)
        extend_banking_audit(self, l2_ids)
        extend_assistant_audit(self,l2_ids)
        extend_bankfeeds_audit(self, l2_ids)
        for company in companies:
            ref = company_ref[company.pk]
            out["companies"].append({"ref": ref, "name": company.name, "currency": company.currency,
                                     "first_run_open": company.first_run_open, "bill_reminder_days": company.bill_reminder_days,
                                     "members": sorted(u.username for u in company.members.all())})
            actor = self.company_actor[ref]
            for account in Account.objects.filter(company=company).order_by("code", "pk"):
                out["accounts"].append({"ref": account_ref.get(account.pk, "?"), "company": ref, "code": account.code, "name": account.name,
                                        "kind": account.kind, "detail_type": account.detail_type, "is_contra": account.is_contra,
                                        "description": account.description, "active": account.active, "postable": account.postable})
                register = reports.register(actor, company, account.pk)
                out["registers"][account_ref.get(account.pk, "?")] = {
                    "opening": cents(register["opening"]), "closing": cents(register["closing"]),
                    "rows": [[journal_ref.get(row["line"].journal_id, "?"), row["line"].position, cents(row["balance"])] for row in register["rows"]]}
            dates = self.scenario.get("as_of") or ["2099-12-31"]
            out["trial_balances"][ref] = {}
            for as_of in dates:
                tb = reports.trial_balance(actor, company, as_of=as_of)
                out["trial_balances"][ref][as_of] = {
                    "rows": [[account_ref.get(row["account"].pk, "?"), cents(row["debit"]), cents(row["credit"])] for row in tb["rows"]],
                    "debit": cents(tb["debit"]), "credit": cents(tb["credit"])}
            def detail_refs(value):
                if isinstance(value, list):
                    return [detail_refs(v) for v in value]
                if isinstance(value, dict):
                    mapped = {}
                    for key, item in value.items():
                        refs = account_ref if key == "account_id" else journal_ref if key == "reversal_id" else None
                        mapped[key] = refs.get(item, f"<unknown:{item}>") if refs is not None else detail_refs(item)
                    return mapped
                return value
            def target_ref(target):
                kind, pk = target.split(":", 1)
                refs = {"company": company_ref, "account": account_ref, "journal": journal_ref}.get(kind, {})
                return f"{kind}:{refs[int(pk)]}" if int(pk) in refs else f"<unknown:{target}>"
            out["audit"][ref] = []
            for event in AuditEvent.objects.filter(company=company).select_related("actor").order_by("pk"):
                row = {"action": event.action, "actor": event.actor.username, "actor_name": event.actor_name,
                       "target": event.target, "detail": event.detail}
                if not (normalize_bankfeeds_audit(row, l2_ids) or normalize_assistant_audit(row,l2_ids) or normalize_attachment_audit(self, row, l2_ids) or normalize_banking_audit(row, l2_ids) or normalize_credits_audit(row, l2_ids) or normalize_pp_audit(row, l2_ids) or normalize_checks_audit(row, l2_ids) or normalize_sales_audit(row, l2_ids) or normalize_payables_audit(row, l2_ids) or normalize_audit(row, l2_ids)):
                    row['target'] = target_ref(row['target'])
                row['detail'] = detail_refs(row['detail'])
                out['audit'][ref].append(row)
        for journal in Journal.objects.filter(company__in=companies).order_by("pk"):
            jref = journal_ref.get(journal.pk, "?")
            out["display"][jref] = str(journal)
            out["journals"].append({
                "ref": jref, "company": company_ref[journal.company_id], "date": str(journal.date), "memo": journal.memo,
                "status": journal.status, "entry_number": journal.entry_number, "adjusting": journal.adjusting, "revision": journal.revision,
                "reversal_of": journal_ref.get(journal.reversal_of_id) if journal.reversal_of_id else None,
                "request_key": str(journal.request_key) if journal.request_key else None,
                "created_by": journal.created_by.username, "posted_by": journal.posted_by.username if journal.posted_by_id else None,
                "lines": [{"position": line.position, "account": account_ref.get(line.account_id, "?"), "debit": cents(line.debit),
                           "credit": cents(line.credit), "description": line.description, "name": line.name}
                          for line in journal.lines.all()]})
        out["l2a"] = payables_l2(self, normalize_l2(self, dump_l2a(self)))
        out["prepayments"] = dump_prepayments(self)
        out["l2a"] = credits_l2(self, out["l2a"])
        out["credits"] = dump_credits(self)
        out["sales"] = dump_sales(self)
        out["payables"] = dump_payables(self)
        out["checks"] = dump_checks(self)
        out["l2a"] = banking_l2(self, out["l2a"])
        out["banking"] = dump_banking(self)
        out["attachments"] = dump_attachments(self)
        if getattr(self, 'backups_results', None):
            out['backups'] = self.backups_results
        selected = backup_identity(self)
        if selected is not None:
            out['book_identity'] = selected
        out["assistant"] = dump_assistant(self)
        financial_reports = dump_reports(self)
        if financial_reports is not None:
            out["financial_reports"] = financial_reports
        out["bankfeeds"] = dump_bankfeeds(self)
        out = bankfeeds_finalize(self, out)
        return out


def main():
    scenario_path, out_path = sys.argv[1], sys.argv[2]
    with open(scenario_path) as handle:
        scenario = json.load(handle)
    with connection.cursor() as cursor:
        cursor.execute("SELECT count(*) FROM ledger_journal")
        if cursor.fetchone()[0]:
            raise SystemExit("the database is not fresh")
    runner = Runner(scenario)
    try:
        runner.run()
        with open(out_path, "w") as handle:
            json.dump(runner.dump(), handle, indent=1, ensure_ascii=False, sort_keys=False)
    finally:
        cleanup_backups(runner)


if __name__ == "__main__":
    main()
