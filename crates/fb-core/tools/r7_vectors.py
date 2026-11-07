"""Known-secret fuzz vectors checked by independent decoders (FIX7).

Writes src/bankfeeds/tests/r7_vectors.json: each vector is a secret and a text that spells it through
one or two encoding layers. Every spelling is confirmed with Python's own decoders before it is kept:
html.unescape, urllib.parse.unquote, json.loads and base64. The Rust test
`r7_independent_decoder_vectors` masks each text; tools/r7_verify.py then decodes the masked texts with
the same decoders and confirms no secret is left.

Run from crates/fb-core: python3 tools/r7_vectors.py
"""
import base64
import html
import html.entities
import json
import random
from collections import Counter
from pathlib import Path
from urllib.parse import unquote

COUNT = 12000
rng = random.Random(0xF1C7)

C1 = {v: k for k, v in html._invalid_charrefs.items() if 0x80 <= k <= 0x9F and ord(v) > 0x9F}
NAMED = {}
for name, value in html.entities.html5.items():
    if len(value) == 1:
        NAMED.setdefault(value, []).append(name)
POOL = (
    [chr(c) for c in range(0x20, 0x7F) if chr(c) != "*"]  # '*' is the mask's own character
    + list(C1)
    + ["é", "ß", "ñ", "Ω", "ж", "中", "文", "😀", "🏦", "́", " ", " "]
)


def secret():
    while True:
        s = "".join(rng.choice(POOL) for _ in range(rng.randint(12, 22)))
        if sum(1 for c in s if not c.isspace()) >= 12:
            return s


def zeros(density):
    """Leading zeros: any number in the first layer, a few in an outer one (which would pad every
    digit of the inner layer's padding)."""
    if density < 1.0:
        return "0" * rng.choice([0, 1, 2])
    return "0" * rng.choice([0, 0, 0, 1, 2, 5, 9, 10, 11, 15, 30, rng.randint(0, 120)])


def html_layer(s, stats, density):
    out = []
    for c in s:
        # a raw `&` left by an outer layer would be read with what follows it
        if rng.random() >= density and c != "&":
            out.append(c)
            continue
        k = rng.random()
        # `;` left out in the first layer only: an outer layer's references sit among the inner one's digits
        semi = "" if rng.random() < 0.25 and density == 1.0 else ";"
        if c in C1 and k < 0.5:
            v = C1[c]
            out.append(f"&#{zeros(density)}{v}{semi}" if rng.random() < 0.5 else f"&#{rng.choice('xX')}{zeros(density)}{v:x}{semi}")
            stats["html-c1"] += 1
        elif c in NAMED and k < 0.65:
            name = rng.choice(NAMED[c])
            out.append("&" + (name if density == 1.0 or name.endswith(";") else name + ";"))
            stats["html-named"] += 1
        elif k < 0.8:
            out.append(f"&#{zeros(density)}{ord(c)}{semi}")
            stats["html-dec"] += 1
        elif k < 0.95:
            h = f"{ord(c):x}"
            out.append(f"&#{rng.choice('xX')}{zeros(density)}{h.upper() if rng.random() < 0.5 else h}{semi}")
            stats["html-hex"] += 1
        else:
            out.append(c)
    return close_references(out)


def close_references(tokens):
    """A reference left without `;` keeps reading the letters and digits after it: give it the `;`
    where the next token starts with one (or with `;`)."""
    out = []
    for i, t in enumerate(tokens):
        nxt = tokens[i + 1][:1] if i + 1 < len(tokens) else " "
        if t.startswith("&") and not t.endswith(";") and (nxt.isalnum() or nxt == ";"):
            t += ";"
        out.append(t)
    return "".join(out)


def pct_layer(s, stats, density):
    out = []
    for c in s:
        if rng.random() >= density and c != "%":
            out.append(c)
            continue
        if c.isalnum() and c.isascii() and rng.random() < 0.3:
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
        k = rng.random()
        if k < 0.5:
            units = c.encode("utf-16-be")
            for i in range(0, len(units), 2):
                h = units[i:i + 2].hex()
                out.append("\\u" + (h.upper() if rng.random() < 0.5 else h))
                stats["json-u"] += 1
        else:
            out.append(json.dumps(c, ensure_ascii=rng.random() < 0.5)[1:-1])
    return "".join(out)


def b64_layer(s, stats, density):
    pre = bytes(rng.randrange(256) for _ in range(rng.randint(0, 2)))
    post = bytes(rng.randrange(256) for _ in range(rng.randint(0, 2)))
    url = rng.random() < 0.5
    enc = (base64.urlsafe_b64encode if url else base64.b64encode)(pre + s.encode() + post).decode()
    stats["b64url" if url else "b64"] += 1
    return enc.rstrip("=") if rng.random() < 0.5 else enc


ENCODE = {"html": html_layer, "pct": pct_layer, "json": json_layer, "b64": b64_layer}


def decode(layer, t):
    if layer == "html":
        return html.unescape(t)
    if layer == "pct":
        return unquote(t)
    if layer == "json":
        return json.loads('"' + t + '"')
    raise ValueError(layer)


def b64_holds(text, s):
    want = s.encode()
    for chunk in text.split():
        for start in range(4):
            body = chunk[start:].replace("-", "+").replace("_", "/").rstrip("=")
            try:
                if want in base64.b64decode(body + "=" * (-len(body) % 4)):
                    return True
            except ValueError:
                pass
    return False


CHAINS = [["html"], ["pct"], ["json"], ["b64"], ["html", "pct"], ["pct", "pct"], ["html", "json"], ["json", "pct"], ["pct", "html"], ["html", "html"]]


def main():
    stats = Counter()
    vectors = []
    while len(vectors) < COUNT:
        if stats["rejected"] > COUNT * 5:
            raise SystemExit(f"too many rejected spellings: {stats}")
        s = secret()
        chain = CHAINS[len(vectors) % len(CHAINS)]
        t = s
        mine = Counter()
        # the first layer spells every character; an outer layer about a third of them, so the
        # nested texts stay short enough to commit
        for depth, layer in enumerate(chain):
            t = ENCODE[layer](t, mine, 1.0 if depth == 0 else 0.3)
        text = f"memo {t} end"
        if chain == ["b64"]:
            ok = b64_holds(text, s)
        else:
            got = text
            try:
                for layer in reversed(chain):
                    got = decode(layer, got)
            except ValueError:
                got = None
            ok = got == f"memo {s} end"
        if not ok:
            stats["rejected"] += 1
            continue
        stats.update(mine)
        stats["/".join(chain)] += 1
        vectors.append({"secret": s, "layers": chain, "text": text})
    out = Path(__file__).resolve().parent.parent / "src/bankfeeds/tests/r7_vectors.json"
    out.write_text(json.dumps(vectors, ensure_ascii=False, separators=(",", ":")) + "\n")
    padded = sum(1 for v in vectors if "&#00" in v["text"] or "&#x00" in v["text"].lower())
    print(json.dumps({"vectors": len(vectors), "with_padded_entities": padded, **stats}, sort_keys=True))


main()
