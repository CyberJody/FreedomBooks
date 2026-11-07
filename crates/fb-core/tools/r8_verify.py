"""Decodes the results of `fix8_independent_decoder_vectors` with Python's own decoders and confirms no
secret is left (FIX8).

    FB_R8_FUZZ_OUT=/path/masked.json cargo test -p fb-core --lib fix8_independent_decoder_vectors
    python3 tools/r8_verify.py /path/masked.json

For each vector the Rust test wrote the composed text (fields masked where they entered, then joined by
the core's composer) and the masked text (what a write stores). The composed text, decoded through the
vector's layers (html.unescape, urllib.parse.unquote, json.loads), must hold the secret: that is the
control that the check can see a leak; a composed text the composer's own `sanitize` cut is counted
apart. The masked text is split at the mask and every piece is decoded the same way (a JSON piece cut
mid-escape: every trim of up to 6 characters at either end that parses); no piece may hold the secret.
Whitespace is ignored in the comparison.
"""
import html
import html.entities
import json
import sys
from collections import Counter
from pathlib import Path
from urllib.parse import unquote

MASK = "***"
TWO = ["&" + k for k, v in html.entities.html5.items() if len(v) > 1]
here = Path(__file__).resolve().parent.parent
vectors = json.loads((here / "src/bankfeeds/tests/r8_vectors.json").read_text())
results = json.loads(Path(sys.argv[1]).read_text())
assert len(vectors) == len(results)


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
            else:
                for a in range(7):
                    for b in range(7):
                        try:
                            nxt.append(json.loads('"' + t[a:len(t) - b] + '"'))
                        except ValueError:
                            pass
        texts = nxt
    return texts


stats = Counter()
leaks = []
for i, (v, r) in enumerate(zip(vectors, results)):
    layers = list(reversed(v["layers"]))
    kind = v["kind"] + "-" + v.get("composer", "text")
    stats[kind] += 1
    stats["/".join(v["layers"])] += 1
    if any(n in v["text"] for n in TWO):
        stats["with two-character references"] += 1
    want = squeeze(v["secret"])
    if any(want in squeeze(t) for t in decodings(layers, r["composed"])):
        stats["control: composed text spells the secret"] += 1
    else:
        stats["control: composer's sanitize cut the spelling"] += 1
    if any(want in squeeze(t) for piece in r["masked"].split(MASK) for t in decodings(layers, piece)):
        leaks.append(i)
print(json.dumps({"vectors": len(vectors), "leaks": len(leaks), "first_leaks": leaks[:10], "stats": stats}, sort_keys=True, indent=1))
sys.exit(1 if leaks else 0)
