# Development checks

Choose focused checks while editing. For a code candidate, `bash scripts/gate.sh --quick` runs workspace clippy, affected-crate tests and isolation. Set `FB_GATE_BASE` to the actual task base. Unknown build/tooling inputs conservatively select the whole workspace. Documentation-only work needs document/link checks, not a product build.

Full release acceptance uses the final integration candidate: the full gate (including JSON feature unification and screen comparison), the relevant differential harness, Electron main/legacy flows and release packaging checks. Run that at the release boundary rather than after every patch. Independent source review can run alongside verification; recheck changed source or newly completed evidence as needed.

For reusable verification, retain the raw output and bind it to the clean commit, toolchain, environment and command:

```bash
python3 scripts/process_evidence.py run --kind quick --output /absolute/durable/quick.json -- bash scripts/gate.sh --quick
python3 scripts/process_evidence.py check --kind quick --output /absolute/durable/quick.json -- bash scripts/gate.sh --quick
```

Supply the expected command independently. Include explicit profile settings in both calls when used. A failed attempt stays failed; a corrected run produces new evidence. Check the failure's cause before retrying, using existing task authority. A documentation-only successor does not turn an earlier run into evidence for a different SHA.

Storage-constrained builds can use `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and `CARGO_INCREMENTAL=0`. These reduce artifacts without disabling assertions or tests. Keep targets isolated by owner, check capacity for cold builds, and retain evidence outside disposable scratch. Local paths and safety floors belong to the environment, not the product.
