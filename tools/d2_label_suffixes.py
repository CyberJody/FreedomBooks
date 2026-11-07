"""Normalize generated label suffixes without rewriting similar text inside a business name/code."""
from pathlib import Path


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-screendiff/src/normalize.rs','''label.find(&marker)''','''label.rfind(&marker)''')
replace('crates/fb-screendiff/src/normalize.rs','''>MOW · #8</option>''','''>MOW #8 · #8</option>''')
replace('crates/fb-screendiff/src/normalize.rs','''>MOW · #2</option>''','''>MOW #8 · #2</option>''')
replace('crates/fb-app/src/screens.rs','''fn customers(book: &mut Book, c: CompanyId)''','''fn short_contact_label(label: String, id: &str, ordinal: usize) -> String {
    let suffix=format!("(#{id})");
    match label.strip_suffix(&suffix) {Some(prefix)=>format!("{prefix}(#{ordinal})"),None=>label}
}

fn customers(book: &mut Book, c: CompanyId)''')
replace('crates/fb-app/src/screens.rs','''fb_core::party_display::contact_list_label(r, &dups, None, "").replace(&format!("(#{id})"),&format!("(#{})",ordinals[id]))''','''short_contact_label(fb_core::party_display::contact_list_label(r, &dups, None, ""),id,ordinals[id])''')
replace('crates/fb-app/src/screens.rs','''text.replace(&format!("(#{id})"),&format!("(#{})",ordinals[id]))''','''short_contact_label(text.to_owned(),id,ordinals[id])''')
