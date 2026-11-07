//! fb-app screens end to end: a GET render (`screens::render` with path and query string) and a
//! form POST (`screens::submit` with decoded fields) against one live encrypted book, plus the
//! command dispatcher (`App::run` on a parsed command) when the input happens to be command JSON.
//!
//! This is the screen form decoding path: every `clean_*` helper in screens/d2..d8 (amounts, dates,
//! integers, choices, UUID keys, formset management counts, report filters). One book is created
//! once per process and mutated across iterations; a crash's artifact therefore reproduces only
//! when replayed on a fresh book with the same path. Asserts: no panic in the app layer; the book's
//! own integrity check stays clean after every write.
#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use fb_app::{App, Command, screens};
use std::sync::{LazyLock, Mutex};

struct Live {
    _dir: tempfile::TempDir,
    app: App,
    company: String,
    accounts: Vec<String>,
    routes: Vec<String>,
}

static LIVE: LazyLock<Mutex<Live>> = LazyLock::new(|| {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fuzz.fbbook");
    let mut app = App::new();
    let key = zeroize::Zeroizing::new([7u8; 32]);
    let created = app.create_book(&path, &key, "Fuzz Owner", "Fuzz Company").expect("create book");
    let company = created.company_id.to_string();
    let open = app.open_mut().unwrap();
    let (book, user) = (&mut open.book, open.owner);
    let mut accounts = Vec::new();
    for (code, kind) in [("1000", "ASSET"), ("2000", "LIABILITY"), ("3000", "EQUITY"), ("4000", "INCOME"), ("5000", "EXPENSE")] {
        let a = fb_core::services::save_account(book, user, created.company_id, fb_core::services::AccountInput::new(code, &format!("Fuzz {code}")).kind(kind)).unwrap();
        accounts.push(a.id.to_string());
    }
    let routes: Vec<String> = fb_templates::django::route_table().iter().map(|(_, pattern)| pattern.clone()).collect();
    Mutex::new(Live { _dir: dir, app, company, accounts, routes })
});

#[derive(Arbitrary, Debug)]
struct Input {
    route: u16,
    account: u8,
    query: Vec<(String, String)>,
    fields: Vec<(String, String)>,
    tail: String,
}

fn fill(pattern: &str, live: &Live, account: u8, tail: &str) -> String {
    let mut out = String::new();
    for (i, part) in pattern.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        if let Some(inner) = part.strip_prefix('<').and_then(|p| p.strip_suffix('>')) {
            let (_, arg) = inner.split_once(':').unwrap_or(("str", inner));
            out.push_str(match arg {
                "company_id" => live.company.as_str(),
                "number" => "1",
                _ if arg.ends_with("_id") => live.accounts[account as usize % live.accounts.len()].as_str(),
                _ => tail,
            });
        } else {
            out.push_str(part);
        }
    }
    out
}

fuzz_target!(|input: Input| {
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let live = &mut *live;
    let pattern = live.routes[input.route as usize % live.routes.len()].clone();
    let path = fill(&pattern, live, input.account, &input.tail);
    let target = format!("{path}?{}", screens::urlencode(&input.query));
    let _ = screens::render(&mut live.app, &target);
    let fields: Vec<(String, String)> = input
        .fields
        .iter()
        .take(64)
        .map(|(k, v)| (k.chars().take(100).collect(), v.chars().take(10_000).collect()))
        .collect();
    // Every route, including the estimates list (FUZZ-AUDIT-REPORT.md F-02, artifact crash-d6cbc47d...,
    // fixed on branch fuzz-fix: D4 answers POST to a GET-only route with 405).
    let _ = screens::submit(&mut live.app, &path, &fields);
    // A raw path too, so the resolver sees the fuzzer's own text.
    let _ = screens::render(&mut live.app, &input.tail);
    if let Ok(command) = fb_app::commands::parse_command(&input.tail) {
        if !matches!(command, Command::SubmitForm { .. }) {
            let _ = live.app.run(command);
        }
    }
    if let Ok(query) = fb_app::commands::parse_query(&input.tail) {
        let _ = live.app.query(query);
    }
    if let Ok(open) = live.app.open_mut() {
        let problems = open.book.check().expect("integrity check runs");
        assert!(problems.is_empty(), "book integrity broken after {input:?}: {problems:?}");
    }
});
