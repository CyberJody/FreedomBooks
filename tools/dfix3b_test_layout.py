"""Keep private parser tests after production items, as required by clippy."""
from pathlib import Path
p=Path(__file__).resolve().parents[1]/'crates/fb-app/src/screens/d7b.rs'
t=p.read_text()
start=t.index('#[cfg(test)]\nmod validator_tests {')
end=t.index('pub(super) fn submit(',start)
tests=t[start:end]
p.write_text(t[:start]+t[end:]+'\n'+tests)
