"""DESKTOP-JODY1 item 3: wire the deliberate-divergence rule (normalize/desktop.rs) into the screen diff.

The rule runs on the Django side only; the Rust app gets fixed host details so the approved page is exact.
"""
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]


def replace(path, old, new):
    p = ROOT / path
    t = p.read_text()
    assert t.count(old) == 1, (path, old)
    p.write_text(t.replace(old, new))


N = 'crates/fb-screendiff/src/normalize.rs'
replace(N, 'mod d8;\nuse regex', 'mod d8;\npub mod desktop;\nuse regex')
replace(N, '''pub struct IdMap {
    generated_undo_key: bool,''', '''pub struct IdMap {
    generated_undo_key: bool,
    /// The Django reference side: deliberate desktop divergences (desktop.rs) are applied to it.
    reference: bool,''')
replace(N, '''        IdMap { by_model, labels, generated_undo_key: false }
    }''', '''        IdMap { by_model, labels, generated_undo_key: false, reference: false }
    }
    /// Mark the map as the Django reference side's (see `desktop`).
    pub fn reference(mut self) -> Self {
        self.reference = true;
        self
    }''')
replace(N, '''    let text = WS.replace_all(&text, " ");
    BETWEEN_TAGS.replace_all(&text, "><").trim().to_owned()
}''', '''    let text = WS.replace_all(&text, " ");
    let text = BETWEEN_TAGS.replace_all(&text, "><").trim().to_owned();
    if ids.reference { desktop::django(text) } else { text }
}''')

M = 'crates/fb-screendiff/src/main.rs'
replace(M, '''    let py_map = normalize::IdMap::new(&django_ids);''', '''    let py_map = normalize::IdMap::new(&django_ids).reference();''')
replace(M, '''    app.open_book(&book_path, &key).expect("open scenario book");''', '''    app.open_book(&book_path, &key).expect("open scenario book");
    // Fixed version/OS for "Report a problem", so its approved desktop page compares exactly.
    app.host = fb_app::Host::new(normalize::desktop::VERSION, normalize::desktop::OS);''')

fixtures = ROOT / 'crates/fb-screendiff/src/normalize/fixtures'
fixtures.mkdir(exist_ok=True)
shutil.copyfile('/mnt/Data/Projects/FreedomBooks/staging/dfix3c/screens/django/report-problem.html', fixtures / 'report-problem.django.html')
print('ok')
