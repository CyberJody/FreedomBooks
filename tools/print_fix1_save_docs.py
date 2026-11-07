from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'PRINT.md'
p.write_text(p.read_text()+'''

## PRINT fix 1: protected destinations

PDF and every CSV export share `electron/main/save-file.cjs`. The native Save dialog confirms
regular-file replacement; the main process refuses the entire active application data directory,
`$XDG_DATA_HOME/FreedomBooks` (default `~/.local/share/FreedomBooks`), the open book and its sidecars
and keys, and `.fbbook*` filenames anywhere. It checks chosen and canonical paths, resolves parent
aliases, and refuses all leaf symlinks, dangling links and non-files. The refusal reads:
“Choose a different file name. That location is used by FreedomBooks.”

Output goes to a private 0600 random temporary file in the selected canonical directory, opened
exclusively with `wx`, fsynced, then renamed over the destination. Linux pins the directory by fd
through `/proc/self/fd`; other platforms check its identity before rename. Renaming never follows
a destination symlink. Failure before rename preserves the old destination and removes our temp.
Internal key persistence is not a user-selected output path. There is no desktop backup Save-dialog
handler on this branch; any future one must use this same helper. Chromium downloads remain blocked.

The reviewer's `save-path-test.cjs` and `save-live.cjs` are retained as the adapted
`electron/scripts/save-path-regression.cjs` and `save-live-regression.cjs`: refusal and an unchanged
synthetic book SHA-256 are required. The latter also checks normal and replacement PDF/CSV output.
`electron/tests/save-file.test.cjs` uses real filesystem entries plus injected leaf/parent races,
temp collisions and write/fsync/rename failures. The review's direct-book probe failed before the
fix (`exact-book must refuse`); protected-path regressions now pass.

Fix verification uses `python3 tools/print_gates.py <gate|harness|native|smoke|smoke-register|node-tests|save-path|save-live|cleanup>`.
Its dedicated databases are `fbv2_core_prf` / `fbv2_core_prfref`; artifacts are in `staging/printfix1/`.
The earlier acceptance section above records the original PRINT run, before this fix.
''')
