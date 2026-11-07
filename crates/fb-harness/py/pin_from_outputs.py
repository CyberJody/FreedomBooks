"""Pin every step of the given scenarios to frozen Python's observed outcome (main's rule).

Usage: python3 pin_from_outputs.py HARNESS_OUT_DIR scenario.json [...]

- success -> "expect_error": false (no kind/message);
- refusal -> "expect_error": true, "expect_kind", "expect_message" (exact Python text).
A step is only pinned when Python and Rust agree on ok/kind/message and no existing pin contradicts it;
otherwise it is reported and left unchanged. DoesNotExist/MissingRef refusals are listed (they need an intent).
"""
import json
import os
import sys

out_dir, paths = sys.argv[1], sys.argv[2:]


def outcomes(path):
    with open(path, encoding="utf-8") as f:
        dump = json.load(f)
    return dump.get("final", dump)["outcomes"]


def indent_of(text):
    for line in text.splitlines()[1:]:
        stripped = line.lstrip(" ")
        if stripped:
            return len(line) - len(stripped)
    return 1


problems, needs_intent, totals = [], [], {}
for path in paths:
    name = os.path.basename(path)[:-5]
    with open(path, encoding="utf-8") as f:
        raw = f.read()
    scenario = json.loads(raw)
    py = outcomes(os.path.join(out_dir, f"{name}.python.json"))
    rs = outcomes(os.path.join(out_dir, f"{name}.rust.json"))
    steps = scenario["steps"]
    if not (len(py) == len(rs) == len(steps)):
        problems.append(f"{name}: outcome counts {len(py)}/{len(rs)} for {len(steps)} steps")
        continue
    changed = refusals = 0
    for i, (step, p, r) in enumerate(zip(steps, py, rs)):
        same = p["ok"] == r["ok"] and (p["ok"] or (p.get("kind") == r.get("kind") and p.get("message") == r.get("message")))
        if not same:
            problems.append(f"{name} step {i} {step['op']}: python {p.get('kind')} {p.get('message')!r} / rust {r.get('kind')} {r.get('message')!r}")
            continue
        if "expect_error" in step and bool(step["expect_error"]) == p["ok"]:
            problems.append(f"{name} step {i} {step['op']}: existing expect_error={step['expect_error']} but python ok={p['ok']} ({p.get('message')!r})")
            continue
        if p["ok"]:
            if "expect_kind" in step or "expect_message" in step:
                problems.append(f"{name} step {i}: success step carries a refusal kind/message pin")
                continue
            if step.get("expect_error") is not False:
                step["expect_error"] = False
                changed += 1
            continue
        refusals += 1
        for key, value in (("expect_kind", p["kind"]), ("expect_message", p["message"])):
            if key in step and step[key] != value:
                problems.append(f"{name} step {i}: existing {key}={step[key]!r} but python {value!r}")
                break
        else:
            if not p["message"]:
                problems.append(f"{name} step {i}: empty refusal message")
                continue
            before = dict(step)
            step["expect_error"] = True
            step["expect_kind"] = p["kind"]
            step["expect_message"] = p["message"]
            changed += step != before
            if p["kind"] in ("DoesNotExist", "MissingRef") and not isinstance(step.get("intent"), str):
                needs_intent.append(f"{name} step {i} {step['op']}: {p['kind']} {p['message']!r}")
    totals[name] = (len(steps), refusals, changed)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(scenario, f, indent=indent_of(raw), ensure_ascii=False)
        f.write("\n")

for name, (steps, refusals, changed) in sorted(totals.items()):
    print(f"{name}: {steps} steps, {refusals} refusals, {changed} pins written")
print(f"\n{len(problems)} problems")
for p in problems:
    print("  ", p[:400])
print(f"\n{len(needs_intent)} DoesNotExist/MissingRef refusals need an intent")
for p in needs_intent:
    print("  ", p[:300])
