"""Probe: Python's ledger.services.money() on edge inputs, printed as JSON vectors for the Rust tests.
Run with the reference venv; needs no database (money() is pure)."""
import json
import os
import sys

sys.path.insert(0, os.environ.get("FB_REFERENCE", "/mnt/Data/Projects/FreedomBooks/wt/reference"))
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "config.settings")
import django  # noqa: E402

django.setup()
from django.core.exceptions import ValidationError  # noqa: E402
from ledger.services import money  # noqa: E402

TEXT = ["0", "1", "1.005", "125.005", "2.675", "0.005", "0.004", "0.0049999", "0.015", "-0", "-0.00", "-0.001", "-1", "+5",
        " 7.5 ", "\t8\n", "1_000", "1__0", "_1", "1_", "1e2", "1E-2", "1.2345e1", "5e-3", "4.9e-3", "1e-100", "1e100",
        "99999999999999.99", "99999999999999.994", "99999999999999.995", "100000000000000", "1e14", "9.9999999999999994e13",
        "NaN", "nan", "sNaN", "Infinity", "-Infinity", "inf", "Inf", "", " ", ".", "1.", ".5", "+.5", "-.0", "1..2", "1e", "e1",
        "0x10", "1,000", "not money", "١٢", "１２.５", "\u00a012", "12\u2003", "\u000112", "12\x1c", "0.125", "0.135", "1.995",
        "000123.4500", "1E+2", "12345678901234567890123456789", "0.0000000000000000000000000000001", "3.14159265358979323846264338327950288"]
TEXT += ["1e-18446744073709551618", "125e-18446744073709551619"]
TEXT += [f"{coefficient}e{exponent}" for coefficient in ["0", "-0", "1", "125"] for exponent in ["999999999999999999", "1000000000000000000", "-999999999999999999", "-1000000000000000000", "9999999999999999999", "-9999999999999999999", "-18446744073709551618", "-18446744073709551619"]]
TEXT += [f"{coefficient}e{exponent}" for coefficient in ["0", "0.0", "0.00", "1", "125", "00125.00"] for exponent in ["999999999999999998", "999999999999999999", "1000000000000000000", "-1999999999999999996", "-1999999999999999997", "-1999999999999999998"]]
INTS = [0, 1, 5, -1, 99999999999999, 100000000000000, 10 ** 30]


def run(value):
    try:
        return {"ok": str(money(value))}
    except ValidationError as exc:
        return {"error": exc.messages[0]}


out = {"text": [[v, run(v)] for v in TEXT], "int": [[str(v), run(v)] for v in INTS],
       "other": {"float": run(1.25), "bool": run(True), "none": run(None)}}
print(json.dumps(out, ensure_ascii=False, indent=1))
