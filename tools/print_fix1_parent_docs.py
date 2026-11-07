from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'PRINT.md'
p.write_text(p.read_text()+'''

The destination guard uses `fs.realpathSync.native` before lexical path cleanup. A regression proved
that `alias/../book-key.sealed` must follow the parent symlink before resolving `..`; Node's ordinary
JavaScript realpath implementation can collapse `..` first. Both that form and ordinary parent aliases
now refuse protected locations, without writing to an unintended lexical destination. The regression
failed first, then passed with all 25 Node tests (12 dedicated save-policy tests).
''')
