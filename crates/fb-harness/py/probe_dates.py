"""Generate model/form date vectors from the frozen Python answer key (no database writes)."""
import json
import os
import sys

sys.path.insert(0, os.environ.get("FB_REFERENCE", "/mnt/Data/Projects/FreedomBooks/wt/reference"))
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "config.settings")
import django

django.setup()
from django.core.exceptions import ValidationError
from django.db.models import DateField as ModelDateField
from django.forms import DateField as FormDateField

values = ["2026-10-31", "20261031", "2026-1-5", "2026-W44-5", "2026W445", "2026-W44", "2026W44", "2020-W53-7",
          "2021-W53-1", "2026-W00-1", "2026-W44-0", "2026-W44-8", "0000-W01-1", "9999-W52-7", "0001-W01-1",
          "２０２６-１-５", "٢٠٢٦-١-٥", "２０２６-01-05", "2026-1-5\n", "2026-02-30", "bad", "", " 10/31/26 ",
          "10/31/2026", "1/5/69", "1/5/68", "  Oct   31 2026 ", "Oct 31, 2026", "31 Oct 2026", "31 Oct, 2026",
          "October 31 2026", "October 31, 2026", "31 October 2026", "31 October, 2026", "oCt 31 2026", "Oct 32 2026",
          "Oct 31 26", "Oct, 31 2026", "Oct 31,2026", "2026-1- 5", "２０２６-1-5", "2026-1-５", "1/５/26", "1/5/２６",
          "Oct ５ 2026", "Oct 0５ 2026", "Oct ５ ２０２６", "2026-１-5", "1/３１/26", "Oct ３１ 2026", "\x1c10/31/26\x1c", "Oct\x1c31\x1c2026", "2026-W445", "2026W44-5"]


def run(field, value):
    try:
        result = field(value)
        return {"ok": result.isoformat() if result is not None else None}
    except ValidationError as exc:
        return {"error": "; ".join(exc.messages)}


print(json.dumps({"model": [[v, run(ModelDateField().to_python, v)] for v in values],
                  "form": [[v, run(FormDateField().clean, v)] for v in values]}, ensure_ascii=False, indent=1))
