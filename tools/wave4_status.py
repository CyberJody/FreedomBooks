"""Aggregate verification facts without emitting payloads or synthetic credentials."""
from pathlib import Path
import json
import re

root = Path(__file__).resolve().parents[1]
out = root.parents[1] / "staging/wave4"
summary = {}
for name in ("gate", "screens", "harness", "native", "smoke", "smoke-secfix1", "node-tests"):
    path = out / (name + ".log")
    if not path.exists():
        continue
    text = path.read_text()
    results = re.findall(r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored", text)
    summary[name] = {
        "passed_test_invocations": sum(int(r[1]) for r in results),
        "failed_test_invocations": sum(int(r[2]) for r in results),
        "ignored_test_invocations": sum(int(r[3]) for r in results),
        "test_binaries_complete": len(results),
        "harness_matches": len(re.findall(r"^MATCH —", text, re.M)),
        "harness_mismatches": len(re.findall(r"^(?:MISMATCH|FAIL)", text, re.M)),
        "completion": re.findall(r"^(?:SCREENS MATCH.*|gate: clippy.*|SMOKE(?: [A-Z0-9]+)? OK.*|SMOKE D8.*|database left.*|fb-node ->.*|[#ℹ] pass .*|[#ℹ] fail .*)$", text, re.M),
    }
summary["harness_scenarios_total"] = len(list((root / "crates/fb-harness/scenarios").glob("*.json")))
(out / "verification-summary.json").write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps(summary, indent=2))
