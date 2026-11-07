"""Build scenarios/j-pan-card-near-misses.json from int-l3-combined.json.

v2 refuses payment card numbers in the reference and memo of payment documents (`fb_core::card_numbers`);
the frozen reference has no such rule, so the refusals themselves are Rust-only tests. This scenario pins
the other side of the rule: text that only looks like a card number (failed Luhn, too few or too many
digits, a leading zero or dots) still posts on both sides, through every payment path of the
combined lifecycle. Values are chosen by request key, so each review and its record carry the same text.

    python3 crates/fb-harness/py/gen_j_pan_scenario.py
"""
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent / "scenarios"
SOURCE = HERE / "int-l3-combined.json"
TARGET = HERE / "j-pan-card-near-misses.json"

NEAR_MISSES = [
    "card ending 1111",
    "4111 1111 1111 1112",
    "Visa 411111111111",
    "4111.1111.1111.1111",
    "0000 0000 0000 0000",
    "41111111111111110000",
    "Conf 1234-5678-9012",
    "PO 4111    1111 1111 1112",
]
REFERENCE_AND_MEMO = {"sales_review_receipt", "sales_record_receipt", "sales_review_sale", "sales_record_sale",
                      "pp_review_deposit", "pp_record_deposit", "banking_review", "banking_deposit"}
MEMO = {"pp_review_refund", "pp_record_refund", "credits_review_refund", "credits_refund", "transfers_review",
        "transfers_record", "checks_save", "payables_review_payment", "payables_pay"}
SEPARATORS = set(" -   　‐‑‒–—―−﹣－")


def digit(c):
    return ord(c) - 48 if "0" <= c <= "9" else ord(c) - 0xFF10 if "０" <= c <= "９" else None


def luhn(ds):
    return sum((2 * d - 9 if d > 4 else 2 * d) if i % 2 else d for i, d in enumerate(reversed(ds))) % 10 == 0


def has_card_number(text):
    """An independent copy of the Rust rule, so the generator refuses to emit a value Rust would refuse."""
    i, n = 0, len(text)
    while i < n:
        if digit(text[i]) is None:
            i += 1
            continue
        chain = []
        while True:
            start = i
            while i < n and digit(text[i]) is not None:
                i += 1
            chain.append((start, i))
            j = i
            while j < n and text[j] in SEPARATORS:
                j += 1
            if not (j > i and j < n and digit(text[j]) is not None):
                break
            i = j
        for a in range(len(chain)):
            ds = []
            for s, e in chain[a:]:
                ds += [digit(c) for c in text[s:e]]
                if len(ds) > 19:
                    break
                if len(ds) >= 13 and ds[0] != 0 and luhn(ds):
                    return True
    return False


def main():
    assert has_card_number("4111 1111 1111 1111") and has_card_number("x 4111111111111111 12/27")
    for gap in ("    ", "        ", " - - "):
        assert has_card_number(f"4111{gap}1111 1111 1111")
    assert not any(has_card_number(v) for v in NEAR_MISSES)
    data = json.loads(SOURCE.read_text())
    keys = {}
    touched = 0
    for step in data["steps"]:
        op, values = step["op"], step.get("values")
        if op not in REFERENCE_AND_MEMO | MEMO or not isinstance(values, dict):
            continue
        n = keys.setdefault(values["request_key"], len(keys))
        values["memo"] = f"Memo {NEAR_MISSES[n % len(NEAR_MISSES)]}"
        reference = NEAR_MISSES[(n + 3) % len(NEAR_MISSES)]
        if op in REFERENCE_AND_MEMO:
            values["reference"] = reference
        if op.startswith("pp_") and "deposit" in op:
            values["order_reference"] = NEAR_MISSES[(n + 5) % len(NEAR_MISSES)]
        # A bill payment's reference is its check number when paid by check.
        if op.startswith("payables_") and values.get("method") != "CHECK":
            values["reference"] = reference
        touched += 1
    TARGET.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n")
    print(f"{TARGET.name}: {touched} steps, {len(keys)} request keys")


if __name__ == "__main__":
    main()
