"""Write the layer-1 harness scenarios (stdlib only, deterministic):

    python3 crates/fb-harness/py/gen_scenarios.py crates/fb-harness/scenarios

Edge cases: half-cent rounding, reversal, void (same-day reversal), contra accounts, multiple companies,
entry numbers / retries / revisions. Random: 1,000 journal entries with a fixed seed.
"""
import json
import os
import random
import sys
import uuid

OUT = sys.argv[1] if len(sys.argv) > 1 else "scenarios"
USERS = [{"username": "owner"}, {"username": "clerk"}, {"username": "stranger"}]


def acct(ref, company, code, name, detail=None, kind=None, contra=None, description=None, **extra):
    step = {"op": "save_account", "ref": ref, "company": company, "code": code, "name": name}
    if detail:
        step["detail_type"] = detail
    if kind:
        step["kind"] = kind
    if contra is not None:
        step["is_contra"] = contra
    if description is not None:
        step["description"] = description
    step.update(extra)
    return step


def draft(ref, company, date, memo, lines, **extra):
    step = {"op": "save_draft", "ref": ref, "company": company, "date": date, "memo": memo, "lines": lines}
    step.update(extra)
    return step


def post(journal, company, **extra):
    return dict({"op": "post_journal", "company": company, "journal": journal}, **extra)


def reverse(ref, journal, company, date, reason, **extra):
    return dict({"op": "reverse_journal", "ref": ref, "company": company, "journal": journal, "date": date, "reason": reason}, **extra)


def dr(account, amount, **extra):
    return dict({"account": account, "debit": amount}, **extra)


def cr(account, amount, **extra):
    return dict({"account": account, "credit": amount}, **extra)


def basic_chart(company="c1"):
    return [
        {"op": "create_company", "ref": company, "name": f"Synthetic {company}", "currency": "USD"},
        acct(f"{company}.bank", company, "1000", "Checking", "BANK"),
        acct(f"{company}.ar", company, "1100", "Receivables", "RECEIVABLE"),
        acct(f"{company}.equip", company, "1500", "Equipment", "FIXED_ASSET"),
        acct(f"{company}.accdep", company, "1590", "Accumulated depreciation", "FIXED_ASSET", contra=True),
        acct(f"{company}.card", company, "2100", "Company card", "CREDIT_CARD"),
        acct(f"{company}.equity", company, "3000", "Owner's equity", "EQUITY"),
        acct(f"{company}.draw", company, "3100", "Owner's draw", "EQUITY", contra=True),
        acct(f"{company}.sales", company, "4000", "Sales", "INCOME"),
        acct(f"{company}.cogs", company, "5000", "Materials", "COST_OF_GOODS_SOLD"),
        acct(f"{company}.exp", company, "6000", "Supplies", "EXPENSE"),
        acct(f"{company}.dep", company, "6100", "Depreciation", "EXPENSE"),
    ]


def half_cent():
    s = basic_chart()
    amounts = ["0.005", "125.005", "2.675", "1.995", "0.015", "0.125", "1_000.004", "١٢.٥", "1e2", "4.9e-3", 7, "99999999999999.994"]
    for i, amount in enumerate(amounts):
        step = draft(f"h{i}", "c1", "2026-01-%02d" % (i + 1), f"Half cent {i}", [dr("c1.exp", amount), cr("c1.bank", amount)])
        if amount == "4.9e-3":
            # Rounds to a zero line: test the refusal without posting a missing ref.
            step.update(expect_error=True, expect_kind="ValidationError", expect_message="Constraint “one_positive_side” is violated.")
        s.append(step)
        if not step.get("expect_error", False):
            s.append(post(f"h{i}", "c1"))
    # Refusals: a half-cent below the rounding point is a zero line; floats, negatives and overflow are refused.
    for i, amount in enumerate(["0.004", 1.25, "-0.01", "100000000000000", "NaN", True, None, "1e-18446744073709551618", "125e-18446744073709551619", "0e1000000000000000000", "0e-1999999999999999998"]):
        s.append(draft(f"bad{i}", "c1", "2026-02-01", f"Bad amount {i}", [dr("c1.exp", amount), cr("c1.bank", "1")], expect_error=True))
    # Rounded lines that balance only after rounding, and ones that don't.
    s.append(draft("r1", "c1", "2026-02-02", "Rounds to balance", [dr("c1.exp", "10.004"), dr("c1.cogs", "10.005"), cr("c1.bank", "20.01")]))
    s.append(post("r1", "c1"))
    s.append(draft("r2", "c1", "2026-02-03", "Rounds out of balance", [dr("c1.exp", "0.005"), dr("c1.cogs", "0.005"), cr("c1.bank", "0.01")]))
    s.append(post("r2", "c1", expect_error=True))
    return {"name": "edge-half-cent", "users": USERS, "steps": s, "as_of": ["2026-01-06", "2026-12-31"]}


def reversal():
    s = basic_chart()
    s += [
        draft("j1", "c1", "2026-03-01", "Equipment on the card", [dr("c1.equip", "1200.00", description="Mower"), cr("c1.card", "1200.00", name="Card co")]),
        post("j1", "c1"),
        reverse("j1r", "j1", "c1", "2026-03-15", "Returned the mower"),
        reverse("j1r2", "j1", "c1", "2026-03-16", "Twice", expect_error=True),
        reverse("j1rr", "j1r", "c1", "2026-03-16", "Reverse the reversal", expect_error=True),
        draft("j2", "c1", "2026-03-02", "Draft only", [dr("c1.exp", "5"), cr("c1.bank", "5")]),
        reverse("j2r", "j2", "c1", "2026-03-03", "Draft", expect_error=True),
        post("j2", "c1"),
        reverse("j2early", "j2", "c1", "2026-03-01", "Too early", expect_error=True),
        reverse("j2blank", "j2", "c1", "2026-03-05", "   ", expect_error=True),
        reverse("j2bad", "j2", "c1", "not a date", "Bad date", expect_error=True),
        reverse("j2r", "j2", "c1", "2026-03-31", "  Month-end correction  "),
        draft("j3", "c1", "2026-03-31", "Accrued depreciation", [dr("c1.dep", "75.50"), cr("c1.accdep", "75.50")], adjusting=True),
        post("j3", "c1"),
        reverse("j3r", "j3", "c1", "2026-04-01", "Auto-reverse accrual " + "x" * 300),
        {"op": "save_draft", "journal": "j3r", "company": "c1", "date": "2026-04-01", "memo": "Edit a reversal", "lines": [], "expect_error": True},
        post("j3r", "c1", expect_error=True),
    ]
    return {"name": "edge-reversal", "users": USERS, "steps": s, "as_of": ["2026-03-14", "2026-03-31", "2026-12-31"]}


def void():
    s = basic_chart()
    s += [
        draft("sale", "c1", "2026-05-10", "Counter sale", [dr("c1.bank", "86.40"), cr("c1.sales", "80.00"), cr("c1.ar", "6.40")]),
        post("sale", "c1"),
        # Void = reverse on the original's own date (how every document void posts through _reverse_journal).
        reverse("sale.void", "sale", "c1", "2026-05-10", "Void: rang up twice"),
        draft("buy", "c1", "2026-05-11", "Supplies", [dr("c1.exp", "19.99"), dr("c1.cogs", "0.01"), cr("c1.card", "20.00")]),
        post("buy", "c1"),
        reverse("buy.void", "buy", "c1", "2026-05-11", "Void"),
    ]
    return {"name": "edge-void", "users": USERS, "steps": s, "as_of": ["2026-05-10", "2026-05-31"]}


def contra():
    s = basic_chart()
    s += [
        draft("cap", "c1", "2026-01-01", "Capital", [dr("c1.bank", "5000"), cr("c1.equity", "5000")]), post("cap", "c1"),
        draft("eq", "c1", "2026-01-02", "Buy equipment", [dr("c1.equip", "3000"), cr("c1.bank", "3000")]), post("eq", "c1"),
        draft("dep", "c1", "2026-01-31", "Depreciation", [dr("c1.dep", "50"), cr("c1.accdep", "50")], adjusting=True), post("dep", "c1"),
        draft("draw", "c1", "2026-01-15", "Owner draw", [dr("c1.draw", "300"), cr("c1.bank", "300")]), post("draw", "c1"),
        # A contra account with a balance on its "wrong" side.
        draft("odd", "c1", "2026-01-20", "Odd contra", [dr("c1.accdep", "10"), cr("c1.exp", "10")]), post("odd", "c1"),
        acct("c1.bankcontra", "c1", "1001", "Contra bank", "BANK", contra=True, expect_error=True),
        acct("c1.arcontra", "c1", "1101", "Contra AR", "RECEIVABLE", contra=True, expect_error=True),
        acct("c1.cardcontra", "c1", "2101", "Contra card", "CREDIT_CARD", contra=True, expect_error=True),
        acct("c1.disc", "c1", "4900", "Sales discounts", "INCOME", contra=True, description="Contra income"),
        draft("disc", "c1", "2026-01-25", "Discount", [dr("c1.disc", "12.34"), cr("c1.bank", "12.34")]), post("disc", "c1"),
        # Type changes: within the kind is allowed for used accounts, across kinds is refused.
        {"op": "save_account", "account": "c1.equip", "company": "c1", "code": "1500", "name": "Equipment", "detail_type": "OTHER_ASSET"},
        {"op": "save_account", "account": "c1.equip", "company": "c1", "code": "1500", "name": "Equipment", "detail_type": "LONG_TERM_LIABILITY", "expect_error": True},
        {"op": "save_account", "account": "c1.draw", "company": "c1", "code": "3100", "name": "Owner draw (renamed)", "kind": "EQUITY"},
        {"op": "save_account", "account": "c1.dep", "company": "c1", "code": "6100", "name": "Depreciation", "kind": "INCOME", "expect_error": True},
        {"op": "save_account", "account": "c1.cogs", "company": "c1", "code": "5000", "name": "Materials (unused)", "kind": "INCOME"},
        acct("c1.spare", "c1", "1999", "Spare", "OTHER_ASSET"),
        {"op": "save_account", "account": "c1.spare", "company": "c1", "code": "2999", "name": "Spare", "detail_type": "LONG_TERM_LIABILITY"},
        acct("c1.kindonly", "c1", "2500", "Payroll liabilities", kind="LIABILITY"),
        acct("c1.mismatch", "c1", "2501", "Mismatch", "BANK", kind="INCOME", expect_error=True),
        acct("c1.notype", "c1", "2502", "No type", expect_error=True),
        acct("c1.badtype", "c1", "2503", "Bad type", "NOT_A_TYPE", expect_error=True),
        acct("c1.dup", "c1", "1000", "Duplicate code", "BANK", expect_error=True),
        acct("c1.blank", "c1", "  ", "Blank code", "BANK", expect_error=True),
        acct("c1.long", "c1", "1700", "Long description", "OTHER_ASSET", description="x" * 201, expect_error=True),
    ]
    return {"name": "edge-contra", "users": USERS, "steps": s, "as_of": ["2026-01-15", "2026-01-31"]}


def multi_company():
    s = basic_chart("c1") + basic_chart("c2")
    s += [
        {"op": "create_company", "ref": "c3", "name": "  Clerk's own company ", "currency": "cad", "actor": "clerk"},
        {"op": "create_company", "ref": "bad", "name": "Bad currency", "currency": "usdx", "expect_error": True},
        {"op": "create_company", "ref": "bad2", "name": "", "currency": "12", "expect_error": True},
        {"op": "create_company", "ref": "anon", "name": "Anonymous", "actor": "<anonymous>", "expect_error": True},
        acct("c3.bank", "c3", "1000", "Bank", "BANK", actor="clerk"),
        acct("c3.inc", "c3", "4000", "Income", "INCOME", actor="clerk"),
        draft("a1", "c1", "2026-06-01", "c1 sale", [dr("c1.bank", "100"), cr("c1.sales", "100")]), post("a1", "c1"),
        draft("b1", "c2", "2026-06-01", "c2 sale", [dr("c2.bank", "250"), cr("c2.sales", "250")]), post("b1", "c2"),
        draft("x1", "c1", "2026-06-02", "Cross-company line", [dr("c1.bank", "1"), cr("c2.sales", "1")], expect_error=True),
        draft("x2", "c1", "2026-06-02", "Stranger", [dr("c1.bank", "1"), cr("c1.sales", "1")], actor="stranger", expect_error=True),
        acct("x3", "c1", "9999", "Stranger account", "BANK", actor="stranger", expect_error=True),
        post("b1", "c1", expect_error=True),
        {"op": "edit_company", "company": "c1", "name": "Renamed c1", "currency": "usd"},
        {"op": "edit_company", "company": "c1", "name": "Renamed c1", "currency": "EUR", "expect_error": True},
        {"op": "edit_company", "company": "c3", "name": "Clerk co", "currency": "EUR", "actor": "clerk"},
        {"op": "edit_company", "company": "c3", "name": "Clerk co", "currency": "EUR", "expect_error": True},
        {"op": "add_member", "company": "c2", "user": "clerk"},
        draft("b2", "c2", "2026-06-03", "Clerk posts in c2", [dr("c2.exp", "40"), cr("c2.bank", "40")], actor="clerk"),
        post("b2", "c2", actor="clerk"),
        reverse("b2r", "b2", "c2", "2026-06-04", "Wrong company card", actor="owner"),
        {"op": "save_account", "account": "c1.exp", "company": "c2", "code": "6000", "name": "Steal", "detail_type": "EXPENSE", "expect_error": True},
        acct("c1.inactive", "c1", "6900", "Inactive", "EXPENSE", active=False),
        # Collation probe: PostgreSQL en_US sorts "1000" before "100-1"; code-point order puts "100-1" first.
        acct("c1.dash", "c1", "100-1", "Petty cash", "OTHER_CURRENT_ASSET"),
        acct("c1.lower", "c1", "a1", "Lowercase code", "OTHER_ASSET"),
        acct("c1.upper", "c1", "A-2", "Uppercase code", "OTHER_ASSET"),
        acct("c1.summary", "c1", "6999", "Summary", "EXPENSE", postable=False),
        draft("x4", "c1", "2026-06-05", "Inactive line", [dr("c1.inactive", "1"), cr("c1.bank", "1")], expect_error=True),
        draft("x5", "c1", "2026-06-05", "Summary line", [dr("c1.summary", "1"), cr("c1.bank", "1")], expect_error=True),
        draft("x6", "c1", "2026-06-05", "Missing account", [dr("no-such-account", "1"), cr("c1.bank", "1")], expect_error=True),
        draft("late", "c1", "2026-06-06", "Deactivated before posting", [dr("c1.exp", "3"), cr("c1.bank", "3")]),
        {"op": "save_account", "account": "c1.exp", "company": "c1", "code": "6000", "name": "Supplies", "detail_type": "EXPENSE", "active": False},
        post("late", "c1", expect_error=True),
        {"op": "save_account", "account": "c1.exp", "company": "c1", "code": "6000", "name": "Supplies", "detail_type": "EXPENSE", "active": True},
        post("late", "c1"),
    ]
    return {"name": "edge-multi-company", "users": USERS, "steps": s, "as_of": ["2026-06-01", "2026-12-31"]}


def entry_numbers():
    s = basic_chart()
    lines = [dr("c1.exp", "100.00", description="Expense part"), cr("c1.bank", "100.00", description="Bank part")]
    key = str(uuid.UUID(int=0x6D1F3A520D1E4C6B9A571E5B3F0C2A10))
    s += [
        draft("n1", "c1", "2026-10-31", "First", lines),
        draft("n2", "c1", "2026-10-31", "Second", lines),
        draft("n3", "c1", "2026-10-31", "Custom", lines, entry_number="  FAM2010 "),
        draft("n4", "c1", "2026-10-31", "Next after custom", lines),
        draft("n5", "c1", "2026-10-31", "Width kept", lines, entry_number="0099"),
        draft("n6", "c1", "2026-10-31", "Width kept next", lines),
        draft("n7", "c1", "2026-10-31", "No digits", lines, entry_number="ADJ-A"),
        draft("n8", "c1", "2026-10-31", "Fallback", lines),
        draft("n9", "c1", "2026-10-31", "Duplicate", lines, entry_number="FAM2010", expect_error=True),
        draft("n10", "c1", "2026-10-31", "Too long", lines, entry_number="x" * 21, expect_error=True),
        draft("n11", "c1", "2026-10-31", "Zero width", lines, entry_number="A\u200bB", expect_error=True),
        draft("n12", "c1", "2026-10-31", "Astral", lines, entry_number="\U0001F9FE" * 20),
        draft("n13", "c1", "2026-10-31", "19 digits", lines, entry_number="1" + "0" * 18),
        draft("n14", "c1", "2026-10-31", "18 nines", lines, entry_number="9" * 18),
        draft("n15", "c1", "2026-10-31", "After 18 nines", lines),
        draft("n16", "c1", "2026-10-31", "20 nines", lines, entry_number="9" * 20),
        draft("n17", "c1", "2026-10-31", "After 20 nines", lines),
        {"op": "save_draft", "journal": "n1", "company": "c1", "date": "2026-10-30", "memo": "Edited", "lines": lines, "expected_revision": 1},
        {"op": "save_draft", "journal": "n1", "company": "c1", "date": "2026-10-30", "memo": "Stale", "lines": lines, "expected_revision": 1, "expect_error": True},
        {"op": "save_draft", "journal": "n1", "company": "c1", "date": "2026-10-30", "memo": "Clash", "lines": lines, "entry_number": "2", "expected_revision": 2, "expect_error": True},
        post("n1", "c1", expected_revision=1, expect_error=True),
        post("n1", "c1", expected_revision=2),
        draft("k1", "c1", "2026-10-31", "Keyed", lines, request_key=key),
        draft("k1", "c1", "2026-10-31", "Keyed", lines, request_key="{" + key.upper() + "}"),
        draft("k2", "c1", "2026-10-31", "Keyed but changed", lines, request_key=key, expect_error=True),
        draft("k3", "c1", "2026-10-31", "Bad key", lines, request_key="nope", expect_error=True),
        post("k1", "c1"),
        draft("k1", "c1", "2026-10-31", "Keyed", lines, request_key=key),
        draft("big", "c1", "2026-10-31", "Over the total bound", [dr("c1.exp", "99999999999999.99"), dr("c1.exp", "0.01"), cr("c1.bank", "99999999999999.99"), cr("c1.bank", "0.01")]),
        post("big", "c1", expect_error=True),
        draft("fifty", "c1", "2026-10-31", "Fifty lines", [dr("c1.exp", "0.01", name="Name %d" % n) for n in range(49)] + [cr("c1.bank", "0.49")]),
        post("fifty", "c1"),
        draft("fiftyone", "c1", "2026-10-31", "51 lines", [dr("c1.exp", "0.01")] * 50 + [cr("c1.bank", "0.50")], expect_error=True),
        draft("extra", "c1", "2026-10-31", "Extra field", [dict(dr("c1.exp", "1"), billable=True), cr("c1.bank", "1")], expect_error=True),
        draft("longname", "c1", "2026-10-31", "Long name", [dr("c1.exp", "1", name="n" * 121), cr("c1.bank", "1")], expect_error=True),
        draft("baddate", "c1", "2026-02-30", "Bad date", lines, expect_error=True),
        draft("blankmemo", "c1", "2026-10-31", "", lines, expect_error=True),
        draft("altdate", "c1", "2026-1-5", "Short ISO date", lines),
        draft("one", "c1", "2026-10-31", "One line", [dr("c1.exp", "5")]),
        post("one", "c1", expect_error=True),
        draft("unbal", "c1", "2026-10-31", "Unbalanced", [dr("c1.exp", "5"), cr("c1.bank", "4.99")]),
        post("unbal", "c1", expect_error=True),
        post("n2", "c1"), post("n3", "c1"), post("n12", "c1"), post("altdate", "c1"),
        reverse("n2r", "n2", "c1", "2026-11-01", "Wrong month"),
    ]
    return {"name": "edge-entry-numbers", "users": USERS, "steps": s, "as_of": ["2026-10-31", "2026-11-30"]}


TYPES = ["BANK", "RECEIVABLE", "OTHER_CURRENT_ASSET", "FIXED_ASSET", "OTHER_ASSET", "PAYABLE", "CREDIT_CARD",
         "OTHER_CURRENT_LIABILITY", "LONG_TERM_LIABILITY", "EQUITY", "INCOME", "COST_OF_GOODS_SOLD", "EXPENSE",
         "OTHER_INCOME", "OTHER_EXPENSE"]
NO_CONTRA = {"BANK", "RECEIVABLE", "PAYABLE", "CREDIT_CARD"}
KIND = {"BANK": "ASSET", "RECEIVABLE": "ASSET", "OTHER_CURRENT_ASSET": "ASSET", "FIXED_ASSET": "ASSET", "OTHER_ASSET": "ASSET",
        "PAYABLE": "LIABILITY", "CREDIT_CARD": "LIABILITY", "OTHER_CURRENT_LIABILITY": "LIABILITY", "LONG_TERM_LIABILITY": "LIABILITY",
        "EQUITY": "EQUITY", "INCOME": "INCOME", "OTHER_INCOME": "INCOME", "COST_OF_GOODS_SOLD": "EXPENSE", "EXPENSE": "EXPENSE",
        "OTHER_EXPENSE": "EXPENSE"}


def randomized(seed=20261030, entries=1000):
    rng = random.Random(seed)
    steps = []
    companies = ["r1", "r2", "r3"]
    accounts = {c: [] for c in companies}
    for c in companies:
        steps.append({"op": "create_company", "ref": c, "name": f"Random books {c}", "currency": "USD"})
        for n in range(24):
            detail = TYPES[n % len(TYPES)]
            contra = detail not in NO_CONTRA and rng.random() < 0.15
            code = str(1000 + n * 37) if n % 7 else f"{1000 + n * 37}-{n}"
            ref = f"{c}.a{n}"
            steps.append(acct(ref, c, code, f"Account {n} {detail.lower()}", detail, contra=contra, description=f"Synthetic {n}"))
            accounts[c].append({"ref": ref, "code": code, "detail": detail, "used": False, "active": True, "name": f"Account {n} {detail.lower()}"})
    posted = {c: [] for c in companies}
    drafts = {c: [] for c in companies}
    reversed_ = set()
    numbers = {c: [] for c in companies}

    def amount():
        roll = rng.random()
        if roll < 0.15:
            return "%d.%03d" % (rng.randint(0, 999), rng.randint(0, 999))       # half cents and below
        if roll < 0.2:
            return rng.randint(1, 5000)                                           # integers
        if roll < 0.25:
            return "%d.%d5" % (rng.randint(0, 99), rng.randint(0, 9))            # exact .x5 halves
        return "%d.%02d" % (rng.randint(0, 20000), rng.randint(0, 99))

    def cents_of(value):
        from decimal import Decimal, ROUND_HALF_UP
        return int((Decimal(str(value)).quantize(Decimal("0.01"), rounding=ROUND_HALF_UP)) * 100)

    def balanced_lines(c):
        pool = [a for a in accounts[c] if a["active"]]
        n_debits = rng.randint(1, 3)
        n_credits = rng.randint(1, 2)
        lines, total = [], 0
        for _ in range(n_debits):
            a = rng.choice(pool)
            value = amount()
            if cents_of(value) == 0:
                value = "0.01"
            total += cents_of(value)
            line = dr(a["ref"], value)
            if rng.random() < 0.2:
                line["description"] = rng.choice(["Fuel", "Mulch", "Rent share", "Café supplies ☕", "Refund"])
            if rng.random() < 0.1:
                line["name"] = rng.choice(["Hudson Valley Plants", "Ada Voss", "Sparrow Hardware"])
            lines.append(line)
            a["used"] = True
        split = [total // n_credits] * n_credits
        split[0] += total - sum(split)
        for part in split:
            a = rng.choice(pool)
            lines.append(cr(a["ref"], "%d.%02d" % divmod(part, 100)))
            a["used"] = True
        rng.shuffle(lines)
        return lines

    for i in range(entries):
        c = rng.choice(companies)
        month, day = rng.randint(1, 12), rng.randint(1, 28)
        date = "2026-%02d-%02d" % (month, day)
        ref = f"e{i}"
        extra = {}
        if rng.random() < 0.08:
            extra["entry_number"] = rng.choice([f"ADJ-{i}", f"{i:05d}", f"Q{month}-{i}"])
        if rng.random() < 0.06:
            extra["adjusting"] = True
        if rng.random() < 0.05:
            extra["request_key"] = str(uuid.UUID(int=rng.getrandbits(128)))
        lines = balanced_lines(c)
        roll = rng.random()
        if roll < 0.03:
            lines = lines[:1]                                                    # one line: post refused
        elif roll < 0.06:
            at = next(k for k, line in enumerate(lines) if "credit" in line)
            lines[at] = dict(lines[at], credit="%s" % (cents_of(lines[at]["credit"]) / 100 + 0.02))   # unbalanced
        steps.append(draft(ref, c, date, f"Random entry {i}", lines, **extra))
        r = rng.random()
        if r < 0.75:
            steps.append(post(ref, c))
            posted[c].append((ref, date))
        else:
            drafts[c].append((ref, date))
        # Occasionally edit and post an older draft.
        if drafts[c] and rng.random() < 0.15:
            dref, ddate = drafts[c].pop(rng.randrange(len(drafts[c])))
            steps.append({"op": "save_draft", "journal": dref, "company": c, "date": ddate, "memo": f"Edited {dref}", "lines": balanced_lines(c)})
            steps.append(post(dref, c))
            posted[c].append((dref, ddate))
        # Reversals and voids of earlier posted entries (some refused: twice, too early).
        if posted[c] and rng.random() < 0.1:
            oref, odate = rng.choice(posted[c])
            when = odate if rng.random() < 0.4 else "2026-12-%02d" % rng.randint(1, 28)
            if rng.random() < 0.1:
                when = "2025-12-31"
            steps.append(reverse(f"{oref}.rev{i}", oref, c, when, rng.choice(["Void", "Correction", "Duplicate entry", "Wrong account"])))
            reversed_.add(oref)
        # Account maintenance: rename, deactivate / reactivate, in-kind and cross-kind type changes.
        if rng.random() < 0.03:
            a = rng.choice(accounts[c])
            choice = rng.random()
            step = {"op": "save_account", "account": a["ref"], "company": c, "code": a["code"], "name": a["name"]}
            if choice < 0.3:
                step["detail_type"] = a["detail"]
                step["active"] = not a["active"]
                a["active"] = not a["active"]
            elif choice < 0.6:
                same_kind = [t for t in TYPES if KIND[t] == KIND[a["detail"]]]
                step["detail_type"] = rng.choice(same_kind)
            else:
                step["detail_type"] = rng.choice(TYPES)
                step["active"] = a["active"]
            steps.append(step)
        if rng.random() < 0.01:
            steps.append(draft(f"s{i}", c, date, "Stranger", balanced_lines(c), actor="stranger"))
    # Post every remaining draft at the end (some are refused: unbalanced, one line, inactive accounts).
    for c in companies:
        for dref, _ in drafts[c]:
            steps.append(post(dref, c))
    as_of = ["2026-03-31", "2026-06-30", "2026-09-30", "2026-12-31"]
    return {"name": "random-1000", "seed": seed, "users": USERS, "steps": steps, "as_of": as_of}


def main():
    os.makedirs(OUT, exist_ok=True)
    for scenario in (half_cent(), reversal(), void(), contra(), multi_company(), entry_numbers(), randomized()):
        path = os.path.join(OUT, scenario["name"] + ".json")
        with open(path, "w") as handle:
            json.dump(scenario, handle, indent=1, ensure_ascii=False)
        print(path, len(scenario["steps"]), "steps")


if __name__ == "__main__":
    main()
