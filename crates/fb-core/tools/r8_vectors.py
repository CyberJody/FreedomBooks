"""Known-secret fuzz vectors for FIX8, checked by independent decoders.

Writes src/bankfeeds/tests/r8_vectors.json, two kinds of vector:

- "single": one text that spells the secret through one or two encoding layers, as in r7_vectors.py,
  except that the secrets carry the two-character expansions of HTML5 named references (`fj`, `<` +
  U+20D2, …) and the HTML layer spells them with those references (`&fjlig;`, `&nvlt;`).
- "split": the secret split into two or three provider fields, each encoded on its own through the same
  layers, which a composer of the core joins back together: `label` (`org · name`), `reason`
  (`org · name: reason`), `memo` (`Bank feed: description`), `summary` (`…; SimpleFIN: error`).
  No field spells the secret; the composed text does.

Every vector is confirmed with Python's own decoders before it is kept (html.unescape,
urllib.parse.unquote, json.loads): the composed text, decoded through the layers, holds the secret. The
Rust test `fix8_independent_decoder_vectors` masks each field where it enters, composes, and masks the
composed text the way every write does; tools/r8_verify.py decodes the results with the same decoders.

Run from crates/fb-core: python3 tools/r8_vectors.py
"""
import html
import html.entities
import json
import random
from collections import Counter
from pathlib import Path
from urllib.parse import unquote

SINGLE = 5000
SPLIT = 7000
rng = random.Random(0xF1C8)

C1 = {v: k for k, v in html._invalid_charrefs.items() if 0x80 <= k <= 0x9F and ord(v) > 0x9F}
NAMED = {}
TWO = {}
for name, value in html.entities.html5.items():
    if len(value) == 1:
        NAMED.setdefault(value, []).append(name)
    else:
        TWO.setdefault(value, []).append(name)
TWO_VALUES = sorted(TWO)
POOL = (
    [chr(c) for c in range(0x20, 0x7F) if chr(c) != "*"]  # '*' is the mask's character
    + list(C1)
    + ["é", "ß", "ñ", "Ω", "ж", "中", "😀", "🏦", "́", " "]
)
# composers of the core and the joints they put between fields (Feed::label, store_pull's reasons and
# summary, accept's memo)
COMPOSERS = {
    "label": ["", " · ", ""],
    "reason": ["", " · ", ": ", ""],
    "memo": ["Bank feed: ", ""],
    "summary": ["", "; SimpleFIN: ", ""],
}


def secret(lo=12, hi=22, two=0.5):
    while True:
        chars = []
        for _ in range(rng.randint(lo, hi)):
            if rng.random() < two / 4:
                chars.append(rng.choice(TWO_VALUES))
            else:
                chars.append(rng.choice(POOL))
        s = "".join(chars)
        if sum(1 for c in s if not c.isspace()) >= 12:
            return s


SHORT = [False]  # split fields: few leading zeros, so the composed text fits what the composer keeps


def zeros(density):
    if density < 1.0 or SHORT[0]:
        return "0" * rng.choice([0, 1, 2])
    return "0" * rng.choice([0, 0, 0, 1, 2, 5, 11, 30])


def html_layer(s, stats, density):
    out = []
    i = 0
    while i < len(s):
        pair = s[i:i + 2]
        if pair in TWO and rng.random() < max(density, 0.6):
            out.append("&" + rng.choice(TWO[pair]))  # the name carries its `;`
            stats["html-two"] += 1
            i += 2
            continue
        c = s[i]
        i += 1
        if rng.random() >= density and c not in "&;":
            out.append(c)
            continue
        k = rng.random()
        semi = "" if rng.random() < 0.25 and density == 1.0 else ";"
        if c in C1 and k < 0.5:
            out.append(f"&#{zeros(density)}{C1[c]}{semi}")
            stats["html-c1"] += 1
        elif c in NAMED and k < 0.65:
            name = rng.choice(NAMED[c])
            out.append("&" + (name if name.endswith(";") else name + ";"))
            stats["html-named"] += 1
        elif k < 0.8:
            out.append(f"&#{zeros(density)}{ord(c)}{semi}")
            stats["html-dec"] += 1
        else:
            out.append(f"&#{rng.choice('xX')}{zeros(density)}{ord(c):x}{semi}")
            stats["html-hex"] += 1
    return close_references(out)


def close_references(tokens):
    out = []
    for i, t in enumerate(tokens):
        nxt = tokens[i + 1][:1] if i + 1 < len(tokens) else ";"
        if t.startswith("&") and not t.endswith(";") and (nxt.isalnum() or nxt in ";"):
            t += ";"
        out.append(t)
    if out and out[-1].startswith("&") and not out[-1].endswith(";"):
        out[-1] += ";"  # a field's last reference: the joint after it may start with a letter
    return "".join(out)


def pct_layer(s, stats, density):
    out = []
    for c in s:
        if (rng.random() >= density or (c.isalnum() and c.isascii() and rng.random() < 0.3)) and c != "%":
            out.append(c)
            continue
        for b in c.encode():
            out.append(f"%{b:02X}" if rng.random() < 0.5 else f"%{b:02x}")
            stats["pct-byte"] += 1
    return "".join(out)


def json_layer(s, stats, density):
    out = []
    for c in s:
        if rng.random() >= density and c not in '"\\' and c >= " ":
            out.append(c)
            continue
        if rng.random() < 0.5:
            units = c.encode("utf-16-be")
            for i in range(0, len(units), 2):
                out.append("\\u" + units[i:i + 2].hex())
                stats["json-u"] += 1
        else:
            out.append(json.dumps(c, ensure_ascii=rng.random() < 0.5)[1:-1])
    return "".join(out)


ENCODE = {"html": html_layer, "pct": pct_layer, "json": json_layer}
CHAINS = [["html"], ["pct"], ["json"], ["html", "pct"], ["pct", "pct"], ["html", "json"], ["json", "pct"], ["pct", "html"], ["html", "html"]]


def decode(layer, t):
    if layer == "html":
        return html.unescape(t)
    if layer == "pct":
        return unquote(t)
    return json.loads('"' + t + '"')


def decoded(chain, text):
    try:
        for layer in reversed(chain):
            text = decode(layer, text)
        return text
    except ValueError:
        return None


def encode(chain, s, stats):
    for depth, layer in enumerate(chain):
        s = ENCODE[layer](s, stats, 1.0 if depth == 0 else 0.3)
    return s


def compose(composer, fields):
    joints = COMPOSERS[composer]
    return joints[0] + "".join(f + j for f, j in zip(fields, joints[1:]))


def main():
    stats = Counter()
    vectors = []
    rejected = 0
    while len(vectors) < SINGLE + SPLIT:
        if rejected > (SINGLE + SPLIT) * 5:
            raise SystemExit(f"too many rejected spellings: {stats}")
        n = len(vectors)
        # a rejected spelling moves on to the next chain and composer, so no pair is retried forever
        chain = CHAINS[(n + rejected) % len(CHAINS)]
        mine = Counter()
        if n < SINGLE:
            s = secret()
            text = f"memo {encode(chain, s, mine)} end"
            if decoded(chain, text) != f"memo {s} end":
                rejected += 1
                continue
            vectors.append({"kind": "single", "secret": s, "layers": chain, "text": text})
            stats["single"] += 1
        else:
            composer = list(COMPOSERS)[(n + rejected) % len(COMPOSERS)]
            parts = len(COMPOSERS[composer]) - 1
            pieces = [secret(4, 8, 0.5) for _ in range(parts)]
            SHORT[0] = True
            s = compose(composer, pieces)
            if sum(1 for c in s if not c.isspace()) < 12:
                rejected += 1
                continue
            fields = [encode(chain, p, mine) for p in pieces]
            composed = compose(composer, fields)
            # within what the composer keeps: a memo is cut at 240 characters, a summary at 500
            if len(composed) > (230 if composer == "memo" else 440):
                rejected += 1
                continue
            # the composed text spells the secret; no field does
            if decoded(chain, composed) != s or any(s in (decoded(chain, f) or "") for f in fields):
                rejected += 1
                continue
            vectors.append({"kind": "split", "composer": composer, "secret": s, "layers": chain, "fields": fields, "text": composed})
            stats["split-" + composer] += 1
        stats.update(mine)
        stats["/".join(chain)] += 1
    out = Path(__file__).resolve().parent.parent / "src/bankfeeds/tests/r8_vectors.json"
    out.write_text(json.dumps(vectors, ensure_ascii=False, separators=(",", ":")) + "\n")
    print(json.dumps({"vectors": len(vectors), "rejected": rejected, **stats}, sort_keys=True))


main()
