"""Decodes the masked texts of `r7_independent_decoder_vectors` with Python's own decoders and confirms
no secret is left (FIX7).

    FB_R7_FUZZ_OUT=/path/masked.json cargo test -p fb-core --lib r7_independent_decoder_vectors
    python3 tools/r7_verify.py /path/masked.json

Each masked text is split at the mask; every piece is decoded through the vector's layers in reverse
(html.unescape, urllib.parse.unquote, json.loads; base64 at every alignment of every run). A piece the
mask cut through the middle of an escape may not parse as JSON: then every trim of up to 6 characters
at either end that parses is checked. Whitespace is ignored in the comparison.
"""
import base64
import html
import json
import sys
from collections import Counter
from pathlib import Path
from urllib.parse import unquote

MASK = "***"
here = Path(__file__).resolve().parent.parent
vectors = json.loads((here / "src/bankfeeds/tests/r7_vectors.json").read_text())
masked = json.loads(Path(sys.argv[1]).read_text())
assert len(vectors) == len(masked)


def squeeze(s):
    return "".join(c for c in s if not c.isspace() and c not in "​‌‍⁠﻿­")


def decodings(layers, piece):
    """Every reading of `piece` through `layers` (outermost first)."""
    texts = [piece]
    for layer in layers:
        nxt = []
        for t in texts:
            if layer == "html":
                nxt.append(html.unescape(t))
            elif layer == "pct":
                nxt.append(unquote(t))
            elif layer == "json":
                for a in range(7):
                    for b in range(7):
                        try:
                            nxt.append(json.loads('"' + t[a:len(t) - b] + '"'))
                        except ValueError:
                            pass
        texts = nxt
    return texts


def b64_holds(text, secret):
    want = secret.encode()
    for chunk in text.replace(MASK, " ").split():
        for start in range(4):
            body = chunk[start:].replace("-", "+").replace("_", "/").rstrip("=")
            body = body[: len(body) - len(body) % 4] if len(body) % 4 == 1 else body
            try:
                if want in base64.b64decode(body + "=" * (-len(body) % 4)):
                    return True
            except ValueError:
                pass
    return False


stats = Counter()
leaks = []
for i, (v, out) in enumerate(zip(vectors, masked)):
    layers = list(reversed(v["layers"]))
    stats["/".join(v["layers"])] += 1
    if v["layers"] == ["b64"]:
        leaked = b64_holds(out, v["secret"])
    else:
        want = squeeze(v["secret"])
        leaked = any(want in squeeze(t) for piece in out.split(MASK) for t in decodings(layers, piece))
        # the unmasked text, decoded the same way, must hold it (the check can see a leak)
        assert any(want in squeeze(t) for t in decodings(layers, v["text"])), i
    if leaked:
        leaks.append(i)
print(json.dumps({"vectors": len(vectors), "leaks": len(leaks), "first_leaks": leaks[:10], "by_layers": stats}, sort_keys=True))
sys.exit(1 if leaks else 0)
