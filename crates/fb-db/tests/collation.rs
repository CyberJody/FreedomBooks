#![cfg(all(target_os = "linux", target_env = "gnu"))]
use std::{cmp::Ordering, ffi::CString};
unsafe extern "C" {
    fn strcoll_l(
        a: *const libc::c_char,
        b: *const libc::c_char,
        locale: libc::locale_t,
    ) -> libc::c_int;
}
struct Oracle(libc::locale_t);
impl Oracle {
    fn compare(&self, a: &str, b: &str) -> Ordering {
        let aa = CString::new(a).unwrap();
        let bb = CString::new(b).unwrap();
        unsafe { strcoll_l(aa.as_ptr(), bb.as_ptr(), self.0) }
            .cmp(&0)
            .then_with(|| a.as_bytes().cmp(b.as_bytes()))
    }
}
impl Drop for Oracle {
    fn drop(&mut self) {
        unsafe { libc::freelocale(self.0) }
    }
}
#[test]
fn realistic_alphabet_200k_pairs() {
    let ptr = unsafe {
        libc::newlocale(
            libc::LC_COLLATE_MASK,
            c"en_US.UTF-8".as_ptr(),
            std::ptr::null_mut(),
        )
    };
    assert!(!ptr.is_null());
    let oracle = Oracle(ptr);
    let alphabet:Vec<char>="abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -./&',()#éèêëáàâäåñçöüøæÉÈÊËÁÀÂÄÅÑÇÖÜØÆ".chars().collect();
    let mut state = 20261030u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut corpus = std::collections::BTreeSet::new();
    for c in &alphabet {
        corpus.insert(c.to_string());
    }
    for s in [
        "", "a", "A", "a-b", "ab", "a b", "ab-", "é", "100-1", "1001", "O'Neil", "ONeil", "Alpha",
        "Álpha",
    ] {
        corpus.insert(s.to_string());
    }
    while corpus.len() < 20_000 {
        let s = match corpus.len() % 3 {
            0 => format!(
                "{}{}{}{}{}",
                next() % 10,
                next() % 10,
                ['-', ' ', '.', '/'][(next() % 4) as usize],
                next() % 10,
                next() % 1000
            ),
            1 => {
                let len = 2 + (next() % 24) as usize;
                (0..len)
                    .map(|_| alphabet[(next() as usize) % alphabet.len()])
                    .collect()
            }
            _ => format!(
                "{}{} {}{}",
                alphabet[(next() as usize) % alphabet.len()],
                next() % 100,
                ["Co.", "& Sons", "-East", "Café", "Ångström", "(North)"][(next() % 6) as usize],
                next() % 100
            ),
        };
        corpus.insert(s);
    }
    let mut corpus: Vec<String> = corpus.into_iter().collect();
    corpus.sort_by(|a, b| oracle.compare(a, b));
    let mut pairs: Vec<(usize, usize)> = (0..corpus.len() - 1).map(|i| (i, i + 1)).collect();
    for _ in 0..200_000 {
        pairs.push((
            (next() % corpus.len() as u64) as usize,
            (next() % corpus.len() as u64) as usize,
        ));
    }
    let mut mismatches = vec![];
    let mut ties = 0;
    for (i, j) in &pairs {
        let a = &corpus[*i];
        let b = &corpus[*j];
        let expected = oracle.compare(a, b);
        let got = fb_db::collation::compare(a, b);
        if expected != got && fb_db::collation::punctuation_tie(a, b) {
            ties += 1;
        } else if expected != got {
            mismatches.push(serde_json::json!({"a":a,"b":b,"glibc":format!("{expected:?}"),"icu4x":format!("{got:?}")}));
        }
    }
    println!(
        "{} strings / {} pairs / {} collation ties / {} mismatches",
        corpus.len(),
        pairs.len(),
        ties,
        mismatches.len()
    );
    for mismatch in &mismatches {
        println!("{mismatch}");
    }
    assert!(
        mismatches.is_empty(),
        "STOP: realistic alphabet mismatch; exact strings above"
    );
}

#[test]
fn punctuation_ties_preserve_case_accents_and_letters() {
    for (a, b) in [("1â", "1,â"), ("a-b", "ab")] {
        assert!(fb_db::collation::punctuation_tie(a, b));
    }
    for (a, b) in [("a", "A"), ("e", "é"), ("a", "b")] {
        assert!(!fb_db::collation::punctuation_tie(a, b));
    }
}

#[test]
#[ignore = "Linux glibc diagnostic, deliberately prints all differences without asserting equivalence"]
fn full_glibc_oracle_diagnostic() {
    let ptr = unsafe {
        libc::newlocale(
            libc::LC_COLLATE_MASK,
            c"en_US.UTF-8".as_ptr(),
            std::ptr::null_mut(),
        )
    };
    assert!(!ptr.is_null());
    let oracle = Oracle(ptr);
    let alphabet: Vec<char> = "aAzZ01- .,/&'()#éÉÅıḰ_{}?l:y=\\".chars().collect();
    let mut state = 20261030u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut strings = std::collections::BTreeSet::new();
    for s in [
        "\u{301}[Åı_",
        "Åı",
        "é",
        "E/\u{301}",
        "\u{301}'k",
        ".K",
        "{,?l",
        "l:",
        "}y",
        "y=\\",
        "1â",
        "1,â",
    ] {
        strings.insert(s.to_string());
    }
    while strings.len() < 10_000 {
        let len = 1 + next() % 12;
        strings.insert(
            (0..len)
                .map(|_| alphabet[next() as usize % alphabet.len()])
                .collect::<String>(),
        );
    }
    let mut strings: Vec<_> = strings.into_iter().collect();
    strings.sort_by(|a, b| oracle.compare(a, b));
    let mut differences = 0;
    for (i, j) in (0..strings.len() - 1)
        .map(|i| (i, i + 1))
        .chain((0..200_000).map(|_| {
            (
                next() as usize % strings.len(),
                next() as usize % strings.len(),
            )
        }))
    {
        let (a, b) = (&strings[i], &strings[j]);
        if oracle.compare(a, b) != fb_db::collation::compare(a, b) {
            differences += 1;
            println!(
                "{a:?} / {b:?}; punctuation tie={}",
                fb_db::collation::punctuation_tie(a, b)
            );
        }
    }
    println!("10000 strings / 209999 pairs / {differences} differences");
}
