# fb-harness: scenario runner for tests

A scenario (JSON, under `scenarios/`) lists users and steps in the ledger command vocabulary. The runner applies it to a fresh SQLCipher book through fb-core and can dump the result as normalized JSON. This crate's own tests pin expected results for those scenarios, and other crates' tests use the runner to build books.

This crate began as a differential harness that ran each scenario through the earlier Python app as well and compared the two. That comparison was retired on 2026-11-11 and its Python side removed. Comments elsewhere that say "not copied from Python" or cite a generator under `fb-harness/py/` describe where a table or rule came from; nothing regenerates them now.
