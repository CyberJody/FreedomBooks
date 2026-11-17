# Development checks

Choose focused checks while editing. For a code candidate, `bash scripts/gate.sh --quick` runs workspace clippy, affected-crate tests and isolation. Set `FB_GATE_BASE` to the actual task base. Unknown build/tooling inputs conservatively select the whole workspace. Documentation-only work needs document/link checks, not a product build.

Full release acceptance uses the final integration candidate: the full gate (including JSON feature unification), Electron main/legacy flows and release packaging checks. Run that at the release boundary rather than after every patch. Independent source review can run alongside verification; recheck changed source or newly completed evidence as needed.

Keep the raw output of the checks behind a delivered build, named by the commit it ran on, in a durable folder outside disposable scratch. A failed attempt stays failed; a corrected run produces new output. Check the failure's cause before retrying. Output from one commit is not evidence for a different commit.

Storage-constrained builds can use `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and `CARGO_INCREMENTAL=0`. These reduce artifacts without disabling assertions or tests. Keep targets isolated by owner, check capacity for cold builds, and retain evidence outside disposable scratch. Local paths and safety floors belong to the environment, not the product.
