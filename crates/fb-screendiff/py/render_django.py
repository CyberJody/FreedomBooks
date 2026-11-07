"""Screen-diff answer key: render the ported screens with the frozen Django app.

    FBV2_DB=fbv2_int_d1_screens . crates/fb-harness/py/env.sh
    $FB_PY crates/fb-screendiff/py/render_django.py <company name> <out dir>

The database must already hold the scenario (crates/fb-harness/py/run_scenario.py). Pages are fetched
through Django's test client as the scenario's first user, exactly as a browser would see them. Nothing is
written to the database (GET requests only). Besides one .html file per screen, `ids.json` maps every
database id that can appear in a page to a symbolic name; the comparer rewrites only those ids.
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
from django.test import Client  # noqa: E402
from django.test.utils import setup_test_environment  # noqa: E402

assert settings.DATABASES["default"]["NAME"].startswith(("fbv2_core_kr1", "fbv2_core_pr", "fbv2_int_d5_", "fbv2_int4b_", "fbv2_int_d1_", "fbv2_int_d2_", "fbv2_int_d3_", "fbv2_int_d4_", "fbv2_int_dfix1_")), "refusing to run outside an approved screen-diff database"

from directory.models import Customer, Item  # noqa: E402
from ledger.models import Account, Company, Journal  # noqa: E402
from sales.models import Invoice, Receipt, Estimate  # noqa: E402

SCREENS = json.load(open(os.environ.get("FB_SCREENDIFF_SCREENS", os.path.join(os.path.dirname(__file__), "..", "screens.json"))))


def main(company_name, out):
    setup_test_environment()
    from unittest.mock import patch
    from datetime import date, datetime
    from pathlib import Path
    from zoneinfo import ZoneInfo
    clock = json.loads((Path(__file__).resolve().parents[1] / "clock.json").read_text())
    instant = datetime.fromisoformat(clock["local_noon"])
    local = instant.astimezone(ZoneInfo(clock["zone"]))
    assert local.hour == 12 and local.minute == 0 and local.utcoffset() == instant.utcoffset()
    frozen_day = local.date()
    class DisplayDate(date):
        @classmethod
        def today(cls):
            return cls(frozen_day.year, frozen_day.month, frozen_day.day)

    # Every entry-view module uses this same local-noon presentation clock. The frozen
    # Python services and stored transaction timestamps keep their original behavior.
    for module in ("sales.views", "sales.invoice_views", "sales.invoice_actions",
                   "sales.receipt_views", "sales.sales_receipt_views", "payables.views",
                   "payables.actions", "payables.payment_views", "transfers.views",
                   "credits.views", "prepayments.views", "checks.views", "banking.views"):
        patch(module + ".date", DisplayDate).start()
    patch("ledger.report_views.today", return_value=frozen_day).start()
    patch("django.utils.timezone.now", return_value=instant).start()
    patch("payables.calendar._utcnow", return_value=instant).start()
    patch("banking.reconcile_views.today", return_value=frozen_day).start()
    patch("banking.register_views.today", return_value=frozen_day).start()
    settings.ALLOWED_HOSTS = list(settings.ALLOWED_HOSTS) + ["testserver", "127.0.0.1"]
    # DESKTOP-BACKUPS: an empty, not-yet-created backup folder (never the reference var/backups).
    import shutil
    backup_dir = Path(out).resolve().parent / "django-backups"
    shutil.rmtree(backup_dir, ignore_errors=True)
    settings.FB_BACKUP_DIR = str(backup_dir)
    os.makedirs(out, exist_ok=True)
    company = Company.objects.get(name=company_name)
    user = get_user_model().objects.order_by("pk").first()
    client = Client()
    client.force_login(user)
    ids = {"company": str(company.pk)}
    for member_company in user.companies.all():
        ids['company:' + member_company.name] = str(member_company.pk)
    for account in Account.objects.filter(company=company):
        ids[f"account:{account.code}"] = str(account.pk)
        ids[f"label:account:{account.code}"] = str(account.pk)
    for invoice in Invoice.objects.filter(company=company).select_related("identity"):
        ids[f"invoice:{invoice.identity.number}"] = str(invoice.pk)
    for row in Estimate.objects.filter(company=company).select_related("identity"):
        ids["estimate:" + row.identity.number] = str(row.pk)
    from collections import Counter
    customers = list(Customer.objects.filter(company=company).order_by('pk'))
    names = Counter(customer.display_name for customer in customers)
    for ordinal, customer in enumerate(customers, 1):
        key = f"customer:{customer.display_name}" + (f":#{ordinal}" if names[customer.display_name] > 1 else '')
        ids[key] = str(customer.pk)
        ids['label:' + key] = str(customer.pk)
    for item in Item.objects.filter(company=company):
        ids[f"item:{item.code}"] = str(item.pk)
        ids[f"label:item:{item.code}"] = str(item.pk)
    for journal in Journal.objects.filter(company=company):
        ids[f"label:journal:{journal}"]=str(journal.pk)
        ids[f"journal:{journal}"] = str(journal.pk)
    for receipt in Receipt.objects.filter(company=company).select_related("identity"):
        ids[f"receipt:{receipt.identity.number}"] = str(receipt.pk)
    from banking.models import Reconciliation, Deposit
    for deposit in Deposit.objects.filter(company=company).select_related("identity"):ids["deposit:"+deposit.identity.number]=str(deposit.pk)
    for reconciliation in Reconciliation.objects.filter(company=company).select_related('account'):
        ids[f'reconciliation:{reconciliation.account.code}:{reconciliation.statement_date}'] = str(reconciliation.pk)
    ids['user:' + user.username] = str(user.pk)
    from documents.first_run import propose
    ids['proposal_digest:current'] = propose(company)['digest']
    from ledger.models import AuditEvent
    from ledger.history import local_stamp
    for event in AuditEvent.objects.all():
        ids['audit_local_stamp:' + local_stamp(event.occurred_at)] = local_stamp(event.occurred_at)
        utc = event.occurred_at.strftime('%Y-%m-%d %H:%M:%S')
        ids['audit_utc_stamp:' + utc] = utc
    for journal in Journal.objects.filter(posted_at__isnull=False):  # every company's (J-ATTACH)
        ids['posted_stamp:' + str(journal)] = journal.posted_at.strftime('%Y-%m-%d %H:%M:%S UTC')
    from directory.models import Vendor
    from salestax.models import TaxItem
    from salestax.services import bulk_preview
    for row in Vendor.objects.filter(company=company).order_by('pk'):
        ids['vendor:' + row.display_name] = str(row.pk)
        ids['label:vendor:' + row.display_name] = str(row.pk)
    for row in TaxItem.objects.filter(company=company):
        ids['taxitem:' + row.name] = str(row.pk)
    ids['bulk_digest:current'] = bulk_preview(company)['digest']
    for other in Company.objects.exclude(pk=company.pk):
        for model, field, cls in [('account','code',Account),('customer','display_name',Customer),('item','code',Item),('vendor','display_name',Vendor)]:
            rows = list(cls.objects.filter(company=other).order_by('pk'))
            counts = Counter(getattr(row, field) for row in rows)
            for ordinal, row in enumerate(rows, 1):
                value = getattr(row, field)
                suffix = f':#{ordinal}' if model in ('customer', 'vendor') and counts[value] > 1 else ''
                key = model + ':' + other.name + ':' + value + suffix
                ids[key] = str(row.pk)
                ids['label:' + key] = str(row.pk)
    from payables.models import Bill, BillPayment
    from checks.models import Check
    from credits.models import CreditMemo, CreditRefund
    for model, cls in [('bill', Bill), ('billpayment', BillPayment), ('check', Check), ('creditmemo', CreditMemo), ('creditrefund', CreditRefund)]:
        for row in cls.objects.filter(company=company).select_related('identity'):
            ids[model + ':' + row.identity.number] = str(row.pk)
    for other in Company.objects.exclude(pk=company.pk):
        for model, cls in [('deposit', Deposit), ('bill', Bill), ('billpayment', BillPayment), ('check', Check), ('creditmemo', CreditMemo), ('creditrefund', CreditRefund), ('receipt', Receipt), ('invoice', Invoice), ('transfer', __import__('transfers.models', fromlist=['Transfer']).Transfer)]:
            for row in cls.objects.filter(company=other).select_related('identity'):
                ids[model + ':' + other.name + ':' + row.identity.number] = str(row.pk)
    from ledger.models import JournalLine
    for account in Account.objects.filter(company=company):
        line = JournalLine.objects.filter(account=account, journal__status='POSTED').order_by('journal__date','journal_id','position','pk').first()
        if line:
            ids[f'line:first:{account.code}'] = str(line.pk)
    for line in JournalLine.objects.filter(journal__company=company).select_related('journal'):
        ids[f'line:{line.journal}:{line.position}'] = str(line.pk)
    for rec in Reconciliation.objects.select_related('company', 'account'):
        if rec.company_id != company.pk:
            ids[f'reconciliation:{rec.company.name}:{rec.account.code}:{rec.statement_date}'] = str(rec.pk)
        for key in ['completed_at', 'undone_at']:
            stamp = getattr(rec, key)
            if stamp:
                text = stamp.strftime('%Y-%m-%d %H:%M')
                ids['reconciliation_stamp:' + text] = text
                text = stamp.isoformat(timespec='seconds')
                ids['reconciliation_csv_stamp:' + text] = text
    from documents.models import DocumentIdentity
    for d in DocumentIdentity.objects.filter(company=company):ids["identity:"+d.number]=str(d.pk)
    from sales.models import EstimateRevision, InvoiceRevision, ReceiptReversal
    for model in (EstimateRevision, InvoiceRevision, Receipt, ReceiptReversal):
        for row in model.objects.all():ids[f"sales_stamp:{model.__name__}:{row.pk}"]=row.created_at.strftime('%Y-%m-%d %H:%M:%S')
    for row in Journal.objects.exclude(company=company).select_related('company'):
        key=f"journal:{row.company.name}:{row}"
        ids[key]=str(row.pk);ids['label:'+key]=str(row.pk)
    for row in DocumentIdentity.objects.exclude(company=company).select_related('company'):
        ids[f"identity:{row.company.name}:{row.number}"]=str(row.pk)
    for journal in Journal.objects.exclude(company=company).select_related('company'):
        ids[f"journal:{journal.company.name}:{journal}"] = str(journal.pk)
    from documents.models import DocumentIdentity
    for identity in DocumentIdentity.objects.select_related('company'):
        ids[f"identity:{identity.company.name}:{identity.number}"] = str(identity.pk)
    from django.utils.formats import date_format
    from payables.models import BillRevision, PaymentReversal
    from transfers.models import Transfer, TransferVoid
    from checks.models import CheckRevision
    from banking.models import DepositReversal
    for cls in [CheckRevision, Deposit, DepositReversal, BillRevision, BillPayment, PaymentReversal, Transfer, TransferVoid]:
        for row in cls.objects.all():
            plain = row.created_at.strftime('%Y-%m-%d %H:%M:%S')
            ids['document_stamp:' + plain] = plain
            display = date_format(row.created_at, "DATETIME_FORMAT")
            ids['document_default_stamp:' + display] = display
    # SIM-FIX item 3: the desktop transfer-list filter offers designated banks (plus banks a recorded
    # transfer used) where Django offers every asset account. Django-side only, so an extra account on
    # the desktop side still diffs (normalize/sim_fix.rs).
    from django.db.models import Q
    for row in Company.objects.all():
        used = Transfer.objects.filter(company=row).values_list('from_account_id', 'to_account_id')
        used = {pk for pair in used for pk in pair}
        for account in Account.objects.filter(company=row, kind='ASSET').exclude(Q(designatedbank__setup__company=row) | Q(pk__in=used)).distinct():
            ids[f'transfer_nonbank:{row.name}:{account.code}'] = str(account.pk)
    from credits.models import CreditApplication
    from prepayments.models import DepositApplication, DepositRefund
    for row in CreditApplication.objects.filter(company=company).select_related('credit_memo__identity').order_by('pk'):
        ordinal = CreditApplication.objects.filter(credit_memo=row.credit_memo, pk__lte=row.pk).count()
        ids[f'creditapplication:{row.credit_memo.identity.number}:{ordinal}'] = str(row.pk)
        ids[f'label:creditapplication:{row.credit_memo.identity.number}:{ordinal}'] = str(row.pk)
    for model, cls in [('depositapplication',DepositApplication),('depositrefund',DepositRefund)]:
        for row in cls.objects.filter(company=company).select_related('identity'):
            ids[model+':'+row.identity.number] = str(row.pk)
    for event in AuditEvent.objects.all():
        stamp=event.occurred_at.strftime('%Y-%m-%d %H:%M')
        ids['d5_event_stamp:'+stamp] = stamp
    from prepayments.models import DepositRefund
    for model, cls in [('deposit', Deposit), ('depositrefund', DepositRefund)]:
        for row in cls.objects.exclude(company=company).select_related('identity', 'company'):
            ids[f"{model}:{row.company.name}:{row.identity.number}"] = str(row.pk)
    # D9 bank feeds (one company): connections by label, rules by name, feed accounts by remote id,
    # journal lines by memo and position, and that company's accounts and accepted documents.
    from bankfeeds.models import BankFeedAccount, BankFeedConnection, BankFeedRule
    from checks.models import Check
    from ledger.models import JournalLine
    from transfers.models import Transfer
    for d9 in Company.objects.filter(name__in=['D9 feed books', 'D9 AI books']):
        for row in BankFeedConnection.objects.filter(company=d9):
            ids['bfconnection:' + row.label] = str(row.pk)
        for row in BankFeedRule.objects.filter(company=d9):
            ids['bfrule:' + row.name] = str(row.pk)
        for row in BankFeedAccount.objects.filter(connection__company=d9):
            ids['bfaccount:' + row.remote_id] = str(row.pk)
        for row in Account.objects.filter(company=d9):
            ids[f'account:{d9.name}:{row.code}'] = str(row.pk)
        for line in JournalLine.objects.filter(journal__company=d9).select_related('journal'):
            ids[f'bfline:{line.journal.memo}:{line.position}'] = str(line.pk)
        from attachments.models import Attachment
        from datetime import timezone as dt_timezone
        from payables.models import Bill
        for row in Attachment.objects.filter(company=d9):
            ids['attachment:' + row.original_name] = str(row.pk)
            for stamp in (row.uploaded_at, row.deleted_at):
                if stamp is not None:
                    text = stamp.astimezone(dt_timezone.utc).strftime('%Y-%m-%d %H:%M')
                    ids['attachment_stamp:' + text] = text
        for cls, model in [(Check, 'check'), (Transfer, 'transfer'), (Receipt, 'receipt'), (Bill, 'bill')]:
            for row in cls.objects.filter(company=d9).select_related('identity'):
                ids[f'{model}:{d9.name}:{row.identity.number}'] = str(row.pk)
    # The desktop always stores bank access encrypted (the book's credential vault): render Django's
    # configured branch, with the harness's synthetic key, never a real one.
    settings.FB_SECRETS_KEYS = ('hZ0w0u3j8yA8b0Pq7cX2r0jLQm6iV5n2kT4sW9eYb1E=',)
    for name, screen in SCREENS.items():
        path = screen["path"]
        for key, value in sorted(ids.items(), key=lambda kv: -len(kv[0])):
            path = path.replace("{" + key + "}", value)
        if 'template' in screen:
            from django.template.loader import render_to_string
            from django.http import HttpResponse
            response = HttpResponse(render_to_string(screen['template'], screen['context']))
        elif 'post' in screen:
            fields = {}
            for key,value in screen['post'].items():
                for symbolic,raw in ids.items():key=key.replace('{'+symbolic+'}',raw)
                fields[key]=value
            for key, value in fields.items():
                for symbolic, raw in sorted(ids.items(), key=lambda kv: -len(kv[0])):
                    value = value.replace('{' + symbolic + '}', raw)
                fields[key] = value
            response = client.post(path, fields)
        else:
            # D9: a screen may name the Host the browser used (the assistant page's loopback case).
            response = client.get(path, HTTP_HOST=screen["host"]) if "host" in screen else client.get(path)
        assert response.status_code == screen.get('status', 200), (name, path, response.status_code, response.content[:500])
        with open(os.path.join(out, f"{name}.html"), "wb") as fh:
            fh.write(response['Location'].encode() if 300 <= response.status_code < 400 else response.content)
        print(f"rendered {name}: {path} ({len(response.content)} bytes)")
    with open(os.path.join(out, "ids.json"), "w") as fh:
        json.dump(ids, fh, indent=1, sort_keys=True)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
