//! FIX6: the connection's own credential never reaches a stored row, summary, audit or return value in
//! any encoding of the threat model (L4-BANKFEEDS.md, "Threat model for credentials"), and the reviewer's
//! R6 regressions ported to known secrets. Encoders and the leak check here are written apart from
//! `secrets.rs`: the `base64` crate, `serde_json`, and plain formatting, matched with `aho-corasick`.
use super::*;
use aho_corasick::{AhoCorasick, AhoCorasickBuilder};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};

/// splitmix64: deterministic, seed printed on failure.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len())]
    }
}

/// Whitespace the fuzz puts inside secrets and inserts into spellings.
const SECRET_SPACES: [char; 7] = [' ', '\t', '\n', '\u{A0}', '\u{2003}', '\u{3000}', '\u{2028}'];
/// Inserted into spellings: none of these is escaped by the JSON wrapper, so the spelling stays whole.
const INSERTED: [char; 6] = [' ', '\u{A0}', '\u{2003}', '\u{3000}', '\u{200B}', '\u{2028}'];
const NON_ASCII: [char; 14] = ['é', 'ß', 'Ω', 'ж', '中', '文', '😀', '🏦', '\u{301}', '\u{FF20}', '\u{FE6B}', '\u{200D}', '€', 'ñ'];

fn blank(c: char) -> bool {
    c.is_whitespace() || text::is_space(c) || matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{AD}')
}
fn strip_blanks(s: &str) -> String {
    s.chars().filter(|&c| !blank(c)).collect()
}
/// A random secret: printable ASCII, whitespace and non-ASCII, at least 12 characters that are not
/// whitespace (the shortest credential component a `SecretSet` accepts).
fn random_secret(rng: &mut Rng) -> String {
    loop {
        let len = 12 + rng.below(21);
        let s: String = (0..len)
            .map(|_| match rng.below(10) {
                0..=6 => char::from(0x20 + rng.below(95) as u8),
                7 => rng.pick(&SECRET_SPACES),
                _ => rng.pick(&NON_ASCII),
            })
            .collect();
        if s.chars().filter(|&c| !blank(c)).count() >= MIN_CREDENTIAL_CHARS {
            return s;
        }
    }
}

fn pct_all(s: &str, upper: bool) -> String {
    s.bytes().map(|b| if upper { format!("%{b:02X}") } else { format!("%{b:02x}") }).collect()
}
/// `urllib.parse.quote(s, safe='')`.
fn pct_url(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { char::from(b).to_string() } else { format!("%{b:02X}") }).collect()
}
fn json_inner(s: &str) -> String {
    let j = serde_json::to_string(s).unwrap();
    j[1..j.len() - 1].to_owned()
}
fn json_all_u(s: &str, upper: bool) -> String {
    s.encode_utf16().map(|u| if upper { format!("\\u{u:04X}") } else { format!("\\u{u:04x}") }).collect()
}
fn html_dec(s: &str) -> String {
    s.chars().map(|c| format!("&#{};", c as u32)).collect()
}
fn html_hex(s: &str) -> String {
    s.chars().map(|c| format!("&#x{:x};", c as u32)).collect()
}
/// Python `html.escape(s, quote=True)`.
fn html_min(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#x27;")
}
/// base64 digits of `v` at byte offset `o` that no neighbour changes (any engine, standard alphabet).
fn b64_aligned(v: &[u8], o: usize) -> String {
    let mut data = vec![0xA5u8; o];
    data.extend_from_slice(v);
    let enc = STANDARD_NO_PAD.encode(&data);
    let start = (o * 8).div_ceil(6);
    let end = data.len() * 8 / 6;
    enc[start..end].to_owned()
}

/// Every spelling of the threat model for one value, labelled.
fn spellings(v: &str) -> Vec<(&'static str, String)> {
    let pct = pct_all(v, true);
    let mut out = vec![
        ("raw", v.to_owned()),
        ("pct-upper", pct.clone()),
        ("pct-lower", pct_all(v, false)),
        ("pct-url", pct_url(v)),
        ("pct-twice-upper", pct.replace('%', "%25")),
        ("pct-twice-lower", pct_all(v, false).replace('%', "%25")),
        ("pct-url-twice", pct_url(v).replace('%', "%25")),
        ("json", json_inner(v)),
        ("json-slash", json_inner(v).replace('/', "\\/")),
        ("json-u-lower", json_all_u(v, false)),
        ("json-u-upper", json_all_u(v, true)),
        ("html-dec", html_dec(v)),
        ("html-hex", html_hex(v)),
        ("html-escape", html_min(v)),
        ("base64", STANDARD.encode(v)),
        ("base64-nopad", STANDARD_NO_PAD.encode(v)),
        ("base64url", URL_SAFE.encode(v)),
        ("base64url-nopad", URL_SAFE_NO_PAD.encode(v)),
    ];
    // inside a longer encoded text, the value starting one or two bytes into a 3-byte group
    out.push(("base64-offset1", STANDARD.encode(format!("x{v}"))));
    out.push(("base64-offset2", URL_SAFE_NO_PAD.encode(format!("xy{v}"))));
    out
}
/// Needles of the leak check: every spelling (whitespace removed) plus every base64 alignment's own
/// digits.
fn needles(values: &[String]) -> Vec<String> {
    let mut out = vec![];
    for v in values {
        out.extend(spellings(v).into_iter().map(|(_, s)| strip_blanks(&s)));
        out.push(STANDARD_NO_PAD.encode(v));
        for o in 0..3 {
            out.push(b64_aligned(v.as_bytes(), o));
            out.push(b64_aligned(v.as_bytes(), o).replace('+', "-").replace('/', "_"));
        }
    }
    out.retain(|n| n.chars().count() >= 8);
    out.sort();
    out.dedup();
    out
}
fn checker(needles: &[String]) -> AhoCorasick {
    AhoCorasickBuilder::new().ascii_case_insensitive(true).build(needles).unwrap()
}
/// Every string leaf of a JSON text, and the text itself.
fn texts_of(s: &str, out: &mut Vec<String>) {
    out.push(s.to_owned());
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        leaves(&v, out);
    }
    // a JSON object inside a text (the fuzz's JSON wrapper), decoded too
    if let (Some(a), Some(b)) = (s.find('{'), s.rfind('}'))
        && a < b
        && let Ok(v) = serde_json::from_str::<Value>(&s[a..=b])
    {
        let mut inner = vec![];
        leaves(&v, &mut inner);
        for t in inner {
            texts_of(&t, out);
        }
    }
}
fn leaves(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| leaves(x, out)),
        Value::Object(o) => o.iter().for_each(|(k, x)| {
            out.push(k.clone());
            leaves(x, out)
        }),
        _ => {}
    }
}
/// Every TEXT value of every table, JSON leaves included.
fn stored_texts(f: &Fx) -> Vec<String> {
    let mut out = vec![];
    for (_, rows) in fb_db::test_support::snapshot(f.book.connection()).unwrap() {
        for row in rows {
            for v in row {
                if let fb_db::rusqlite::types::Value::Text(s) = v {
                    texts_of(&s, &mut out);
                }
            }
        }
    }
    out
}
fn leaks(ac: &AhoCorasick, needles: &[String], texts: &[String]) -> Vec<String> {
    let mut found = vec![];
    for t in texts {
        if let Some(m) = ac.find(&strip_blanks(t)) {
            found.push(needles[m.pattern().as_usize()].clone());
        }
    }
    found
}

/// Whitespace inserted at random places, inside escapes too.
fn insert_blanks(rng: &mut Rng, s: &str) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    for _ in 0..1 + rng.below(3) {
        let at = rng.below(chars.len() + 1);
        chars.insert(at, rng.pick(&INSERTED));
    }
    chars.into_iter().collect()
}
fn wrap(rng: &mut Rng, s: &str) -> String {
    match rng.below(5) {
        0 => s.to_owned(),
        1 => format!("Error: '{s}' end"),
        2 => json!({"v": s}).to_string(),
        3 => format!("<p>{s}</p>"),
        _ => format!("https://bridge.test/x?q={s}"),
    }
}

/// One fuzz case: a connection credential and its spellings, assigned to the provider fields.
struct Case {
    secrets: SecretSet,
    values: Vec<String>,
    fields: Vec<String>,
}
const FIELDS: [&str; 14] = [
    "host", "failure", "conn_key", "account_id", "name", "org", "currency", "account_skip", "pull_skip", "provider_error", "tx_id", "tx_description", "staged_id", "staged_description",
];
fn fuzz_case(rng: &mut Rng, n: usize, spelled: &mut std::collections::BTreeMap<String, usize>) -> Case {
    let (user, pass) = (random_secret(rng), random_secret(rng));
    let access = format!("https://{}:{}@bridge{n}.example.test/simplefin", pct_url(&user), pct_url(&pass));
    let secrets = SecretSet::masking_from_access_url(&access, Vec::<String>::new()).unwrap();
    let pair = format!("{user}:{pass}");
    let mut pieces: Vec<String> = vec![];
    for v in [&user, &pass, &pair] {
        for (label, s) in spellings(v) {
            *spelled.entry(label.to_owned()).or_default() += 2;
            pieces.push(s.clone());
            pieces.push(insert_blanks(rng, &s));
        }
    }
    // every piece lands in one field; the rotation moves each spelling through every field over cases
    let mut bundles = vec![vec![]; FIELDS.len()];
    for (i, p) in pieces.into_iter().enumerate() {
        bundles[(i + n) % FIELDS.len()].push(p);
    }
    // whole access-URL forms are long even when masked: they go to the fields without a short bound
    let long = ["failure", "provider_error", "tx_description", "staged_description"];
    let extras = [access.clone(), format!("Authorization: Basic {}", STANDARD.encode(&pair)), json_inner(&access).replace('/', "\\/"), pct_url(&access).replace('%', "%25")];
    for (i, extra) in extras.into_iter().enumerate() {
        *spelled.entry("access-url-forms".into()).or_default() += 1;
        bundles[FIELDS.iter().position(|f| *f == long[(i + n) % long.len()]).unwrap()].push(extra);
    }
    let fields = bundles.into_iter().enumerate().map(|(k, b)| format!("{}{k}-{n} {}", FIELDS[k], wrap(rng, &b.join(" ")))).collect();
    Case { secrets, values: vec![user, pass, pair], fields }
}

/// Runs `cases` fuzz cases from `seed` through every bank-feed entry point that takes provider text,
/// and the services that write it on (mapping, acceptance, ignore, return), in books of 64 cases.
fn known_secret_fuzz(seed: u64, cases: usize) -> (usize, usize, std::collections::BTreeMap<String, usize>, std::collections::BTreeSet<String>) {
    let mut rng = Rng(seed);
    let mut spelled = std::collections::BTreeMap::new();
    let mut actions = std::collections::BTreeSet::new();
    let mut checked = 0;
    let mut n = 0;
    while n < cases {
        let mut f = Fx::new();
        let mut all_needles = vec![];
        for _ in 0..64.min(cases - n) {
            let case = fuzz_case(&mut rng, n, &mut spelled);
            let s = &case.secrets;
            let fv = |k: &str| case.fields[FIELDS.iter().position(|x| *x == k).unwrap()].clone();
            let mut returned: Vec<String> = vec![];
            let connection = connect(&mut f.book, f.u, f.c, &json!("F6"), &fv("host"), s).unwrap();
            returned.push(connection.to_string());
            returned.push(forget(&mut f.book, f.u, f.c, connection["id"].as_str().unwrap(), &SecretSet::none()).unwrap().to_string());
            record_sync_failure(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, &fv("failure"), n % 2 == 0, s).unwrap();
            let mut a = acct(&fv("account_id"), &fv("name"), vec![tx(&fv("tx_id"), "2026-10-20", 100, &fv("tx_description"))]);
            a.conn_key = fv("conn_key");
            a.org_name = fv("org");
            a.skipped = vec![fv("account_skip")];
            let mut other = acct(&format!("f6-other-{n}"), "Other", vec![]);
            other.currency = fv("currency");
            let pull = StagedPull { accounts: vec![a, other], skipped: vec![fv("pull_skip")], errors: vec![fv("provider_error")] };
            let synced = store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &pull, NOW, false, None, s).unwrap_or_else(|e| {
                let masked: Vec<String> = case.fields.iter().map(|v| staging::provider_text(s, v)).collect();
                panic!("seed {seed:#x} case {n}: {e:?}; masked fields: {masked:#?}; values {:?}; raw {:?}", case.values, case.fields)
            });
            assert_eq!((synced["accounts"].as_i64(), synced["new"].as_i64()), (Some(2), Some(1)), "seed {seed:#x} case {n}");
            returned.push(synced.to_string());
            let staged = stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[(fv("account_id"), tx(&fv("staged_id"), "2026-10-21", -100, &fv("staged_description")))], s).unwrap();
            returned.push(format!("{staged:?}"));
            let refused = stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[(format!("unknown {}", fv("account_id")), tx("x", "2026-10-21", 1, "x"))], s);
            returned.push(format!("{refused:?}"));
            returned.push(f.map(&fv("account_id"), Some("100")).unwrap().to_string());
            let item = f.item(&fv("tx_id"));
            let has_control = !fb_db::check_numbers::free_text_valid(&super::super::accept::memo_text(&item.description))
                || !fb_db::check_numbers::free_text_valid(&text::sanitize(&item.description, 160));
            let before_accept = has_control.then(|| fb_db::test_support::snapshot(f.book.connection()).unwrap());
            match f.accept(&fv("tx_id"), json!({"kind":"JOURNAL","account_id":f.acc("410")})) {
                Ok(accepted) => returned.push(format!("{accepted:?}")),
                Err(e) => {
                    // SEC-FIX1: random spellings may retain an unrelated Cc/Cf
                    // after masking. These cannot become new journal text.
                    assert_eq!(e.message(), "Control and formatting characters are not allowed.");
                    assert_eq!(before_accept.expect("control refusal was predicted from the derived memo/description"), fb_db::test_support::snapshot(f.book.connection()).unwrap(), "a refused control-bearing journal must write nothing");
                    returned.push(e.message());
                }
            }
            let staged_item = f.item(&fv("staged_id"));
            returned.push(format!("{:?}", ignore(&mut f.book, f.u, f.c, &staged_item.id, &SecretSet::none()).unwrap()));
            returned.push(format!("{:?}", return_to_review(&mut f.book, f.u, f.c, &staged_item.id, &SecretSet::none()).unwrap()));
            returned.push(f.map(&fv("account_id"), None).unwrap().to_string());
            let mine = needles(&case.values);
            let found = leaks(&checker(&mine), &mine, &returned);
            assert!(found.is_empty(), "seed {seed:#x} case {n}: a return value holds a spelling of the secret ({} needles)", found.len());
            checked += returned.len();
            all_needles.extend(mine);
            n += 1;
        }
        let texts = stored_texts(&f);
        let found = leaks(&checker(&all_needles), &all_needles, &texts);
        assert!(found.is_empty(), "seed {seed:#x} through case {n}: {} stored texts hold a spelling of a secret", found.len());
        checked += texts.len();
        let mut st = f.book.connection().prepare("SELECT DISTINCT action FROM ledger_auditevent WHERE action GLOB 'bankfeed.*'").unwrap();
        actions.extend(st.query_map([], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap));
        drop(st);
        r5_finish(&mut f);
    }
    (n, checked, spelled, actions)
}
fn run_fuzz(seed: u64) {
    let cases: usize = std::env::var("FB_FIX6_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(1280);
    let (n, checked, spelled, actions) = known_secret_fuzz(seed, cases);
    eprintln!("FIX6 fuzz seed {seed:#x}: {n} cases, {checked} stored/returned texts checked, 0 leaks; spellings embedded: {spelled:?}; actions: {actions:?}");
    // the sync, staging, mapping and review audits all ran (rule audits carry user text only)
    for a in ["bankfeed.connected", "bankfeed.forgotten", "bankfeed.synced", "bankfeed.staged", "bankfeed.mapped", "bankfeed.accepted", "bankfeed.ignored", "bankfeed.returned"] {
        assert!(actions.contains(a), "{a} not driven");
    }
}
// Four seeds of 1,280 cases each (5,120 in all), so the workspace run spreads them over its threads.
#[test]
fn fix6_known_secret_fuzz_a() {
    run_fuzz(0xF16A_0001_5EC2_E7A1);
}
#[test]
fn fix6_known_secret_fuzz_b() {
    run_fuzz(0xF16B_0002_5EC2_E7B2);
}
#[test]
fn fix6_known_secret_fuzz_c() {
    run_fuzz(0xF16C_0003_5EC2_E7C3);
}
#[test]
fn fix6_known_secret_fuzz_d() {
    run_fuzz(0xF16D_0004_5EC2_E7D4);
}

/// The checker itself finds every spelling it is given, so a clean fuzz run means something.
#[test]
fn fix6_checker_finds_every_unmasked_spelling() {
    let mut rng = Rng(0xC4EC_4E70);
    for _ in 0..200 {
        let v = random_secret(&mut rng);
        let mine = needles(std::slice::from_ref(&v));
        let ac = checker(&mine);
        for (label, s) in spellings(&v) {
            for text in [s.clone(), insert_blanks(&mut rng, &s), json!({"v": s}).to_string()] {
                let mut texts = vec![];
                texts_of(&format!("x {text} y"), &mut texts);
                assert!(!leaks(&ac, &mine, &texts).is_empty(), "{label}: {text:?}");
            }
        }
        for o in 0..3 {
            let pre = ["", "a", "ab"][o];
            let text = STANDARD.encode(format!("{pre}{v}tail"));
            assert!(!leaks(&ac, &mine, std::slice::from_ref(&text)).is_empty(), "base64 at offset {o}: {text}");
        }
        // and the masker leaves none of them
        let set = SecretSet::masking_new([v.clone()]).unwrap();
        for (label, s) in spellings(&v) {
            for text in [s.clone(), insert_blanks(&mut rng, &s), wrap(&mut rng, &s)] {
                let masked = set.mask(&format!("x {text} y"));
                let mut texts = vec![];
                texts_of(&masked, &mut texts);
                assert!(leaks(&ac, &mine, &texts).is_empty(), "{label}: {text:?} -> {masked:?}");
            }
        }
    }
}

// --- the reviewer's R6 regressions, ported to known secrets ----------------------------------------

fn r6_enc(s: &str) -> String {
    s.bytes().map(|b| format!("%{b:02X}")).collect()
}
fn r6_leaks(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    ["R6SECRETUSER", "R6SECRETPASS"]
        .iter()
        .any(|m| s.contains(&m.to_ascii_lowercase()) || s.contains(&r6_enc(m).to_ascii_lowercase()) || s.contains(&r6_enc(&r6_enc(m)).to_ascii_lowercase()))
}
/// R6's cases, each with the user name and password the plugin holds for it.
struct R6Case {
    name: String,
    url: String,
    secrets: SecretSet,
}
fn r6_cases() -> Vec<R6Case> {
    let mut pool: Vec<char> = (32u8..=126).map(char::from).collect();
    pool.extend(['é', '中', '😀', '\u{301}', '\u{a0}', '\u{2003}', '\u{2028}', '\u{200d}', '\u{ff20}', '\u{fe6b}']);
    let mut out = vec![];
    for (i, c) in pool.into_iter().enumerate() {
        for mode in 0..3 {
            let (user, pass) = (format!("R6SECRETUSER{i}{c}tail"), format!("R6SECRETPASS{i}{c}tail"));
            let raw = format!("{user}:{pass}");
            let credentials = match mode {
                0 => raw,
                1 => r6_enc(&raw),
                _ => raw
                    .replace(' ', "%20")
                    .replace('/', "%2F")
                    .replace('?', "%3F")
                    .replace('#', "%23")
                    .replace('\u{a0}', "%C2%A0")
                    .replace('\u{2003}', "%E2%80%83")
                    .replace('\u{2028}', "%E2%80%A8"),
            };
            out.push(R6Case {
                name: format!("U+{:04X}/mode{mode}", c as u32),
                url: format!("https://{credentials}@r6.test/{i}/{mode}"),
                secrets: SecretSet::masking_from_credentials(&user, &pass, Vec::<String>::new()).unwrap(),
            });
        }
    }
    let plain = "https://R6SECRETUSER:RD6:R6SECRETPASS@r6.test/encoded";
    let known = || SecretSet::masking_from_credentials("R6SECRETUSER", "RD6:R6SECRETPASS", Vec::<String>::new()).unwrap();
    for (name, url) in [
        ("encoded-url-once", r6_enc(plain)),
        ("encoded-separators-twice", "https%253a%252f%252fR6SECRETUSER:RD6:R6SECRETPASS%2540r6.test/double".into()),
        ("JSON-escaped-slashes", plain.replace('/', "\\/")),
        ("JSON-escaped-separators", plain.replace(':', "\\u003a").replace('/', "\\u002f").replace('@', "\\u0040")),
    ] {
        out.push(R6Case { name: name.into(), url, secrets: known() });
    }
    out
}
fn r6_wrappers(url: &str) -> [String; 5] {
    [url.to_owned(), format!("Failure '{url}'"), json!({"url":url}).to_string(), format!("<b>{url}</b>"), format!("\u{301}x{url}")]
}

/// R6 pure-text cases with the plugin's secrets: the masked provider text and every text helper after it.
#[test]
fn r6_known_secret_whitespace_json_and_double_encoding_all_text_helpers() {
    let mut failures = vec![];
    let mut count = 0;
    for case in r6_cases() {
        for wrapper in r6_wrappers(&case.url) {
            let entered = staging::provider_text(&case.secrets, &wrapper);
            for (path, out) in [
                ("provider_text", entered.clone()),
                ("sanitize", case.secrets.mask(&text::sanitize(&entered, 500))),
                ("strip", text::strip_url_userinfo(&entered)),
                ("mask-alone", case.secrets.mask(&wrapper)),
            ] {
                count += 1;
                if r6_leaks(&out) {
                    failures.push(format!("{}/{path}", case.name));
                }
            }
        }
    }
    eprintln!("R6 known-secret pure-text checks={count}, leaks={}", failures.len());
    assert!(failures.is_empty(), "R6 credential leaks with a known secret: {failures:?}");
}

/// Leaks of the structural backstop on R6's unknown secrets at FIX6 (outside the guarantee).
const R6_BACKSTOP_LEAKS: usize = 105;

/// R6's original pure-text test: the structural rule alone, with the secret UNKNOWN to the book.
/// Threat model (L4-BANKFEEDS.md): an unknown credential-looking URL gets best-effort structural
/// redaction only; whitespace inside userinfo, JSON-escaped separators and double-encoded separators
/// are outside the guarantee there (review 6 counted 105 such leaks). FIX7: runs in the gate as a
/// ratchet instead of being ignored: the backstop may get better, never worse.
#[test]
fn r6_fuzz_raw_and_encoded_credential_contents_all_text_helpers() {
    let mut failures = vec![];
    let mut count = 0;
    for case in r6_cases() {
        for wrapper in r6_wrappers(&case.url) {
            for (path, out) in [("redact", text::redact(&wrapper)), ("sanitize", text::sanitize(&wrapper, 500)), ("strip", text::strip_url_userinfo(&wrapper))] {
                count += 1;
                if r6_leaks(&out) {
                    failures.push(format!("{}/{path}", case.name));
                }
            }
        }
    }
    eprintln!("R6 pure-text checks={count}, leaks={} cases={failures:?}", failures.len());
    assert_eq!(count, 4785, "the R6 cases changed");
    assert!(failures.len() <= R6_BACKSTOP_LEAKS, "the unknown-secret backstop got worse: {} leaks in {count} pure-text checks", failures.len());
}

/// R6's persisted test with each case's secrets passed in: every provider field, every audit writer,
/// every table.
#[test]
fn r6_known_secret_persisted_all_provider_fields_and_audit_writers() {
    let mut failures = std::collections::BTreeSet::new();
    let mut seen = std::collections::BTreeSet::<String>::new();
    let mut n = 0;
    for cases in r6_cases().chunks(45) {
        let mut f = Fx::new();
        let category = f.acc("500");
        let rule = save_rule(&mut f.book, f.u, f.c, &json!({"name":"R6 first","match_text":"office","account_id":category}), &SecretSet::none()).unwrap();
        save_rule(&mut f.book, f.u, f.c, &json!({"name":"R6 second","match_text":"fuel","account_id":category}), &SecretSet::none()).unwrap();
        save_rule(&mut f.book, f.u, f.c, &json!({"rule_id":rule.id,"name":"R6 edited","match_text":"office","account_id":category}), &SecretSet::none()).unwrap();
        move_rule(&mut f.book, f.u, f.c, &rule.id, &json!("down"), &SecretSet::none()).unwrap();
        set_rule_active(&mut f.book, f.u, f.c, &rule.id, &json!(false), &SecretSet::none()).unwrap();
        for case in cases {
            n += 1;
            let (value, s) = (&case.url, &case.secrets);
            let connection = connect(&mut f.book, f.u, f.c, &json!("R6"), value, s).unwrap();
            if r6_leaks(&connection.to_string()) {
                failures.insert(format!("{}/connect-return", case.name));
            }
            forget(&mut f.book, f.u, f.c, connection["id"].as_str().unwrap(), &SecretSet::none()).unwrap();
            let failure = record_sync_failure(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, value, n % 2 == 0, s);
            if r6_leaks(&format!("{failure:?}")) {
                failures.insert(format!("{}/failure-return", case.name));
            }
            failure.unwrap();
            let mut a = acct(value, value, vec![tx(value, "2026-10-20", 100, value)]);
            a.conn_key = value.clone();
            a.org_name = value.clone();
            a.skipped = vec![value.clone()];
            let mut other = acct(&format!("other-{n}"), "Other", vec![]);
            other.currency = value.clone();
            let pull = StagedPull { accounts: vec![a, other], skipped: vec![value.clone()], errors: vec![value.clone()] };
            let result = store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &pull, NOW, false, None, s).unwrap();
            if r6_leaks(&result.to_string()) {
                failures.insert(format!("{}/sync-return", case.name));
            }
            let mapped = f.map(value, Some("100")).unwrap();
            if r6_leaks(&mapped.to_string()) {
                failures.insert(format!("{}/map-return", case.name));
            }
            let staged = format!("{value}-staged");
            stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[(value.clone(), tx(&staged, "2026-10-20", -100, value))], s).unwrap();
            let rejected = stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[(format!("unknown-{value}"), tx("unused", "2026-10-20", 1, value))], s);
            assert!(rejected.is_err());
            if r6_leaks(&format!("{rejected:?}")) {
                failures.insert(format!("{}/stage-error", case.name));
            }
            let item = f.item(value);
            ignore(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap();
            return_to_review(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap();
            let journal = f.accept(value, json!({"kind":"JOURNAL","account_id":f.acc("410")})).unwrap();
            let check = f.accept(&staged, json!({"kind":"CHECK","account_id":f.acc("500")})).unwrap();
            if r6_leaks(&format!("{journal:?}{check:?}")) {
                failures.insert(format!("{}/accept-return", case.name));
            }
            f.map(value, None).unwrap();
            f.reopen_clean();
        }
        let mut st = f.book.connection().prepare("SELECT action,detail FROM ledger_auditevent").unwrap();
        for (action, payload) in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap().map(Result::unwrap) {
            if action.starts_with("bankfeed.") {
                seen.insert(action.clone());
            }
            if r6_leaks(&payload) {
                failures.insert(format!("audit/{action}"));
            }
        }
        drop(st);
        let snapshot = fb_db::test_support::snapshot(f.book.connection()).unwrap();
        for (table, rows) in snapshot {
            if r6_leaks(&format!("{rows:?}")) {
                failures.insert(format!("table/{table}"));
            }
        }
        r5_finish(&mut f);
    }
    assert_eq!(seen, fix4_emitted_actions());
    eprintln!("R6 known-secret persisted cases={n}, fields=15, actions={}, leaks={failures:?}", seen.len());
    assert!(failures.is_empty(), "R6 provider credential leak categories={}", failures.len());
}

/// R6's read-back: the stored summary and the audit right after each failure, with the secret known.
#[test]
fn r6_known_secret_readback_failure_summary_and_audit() {
    let mut f = Fx::new();
    let mut failures = vec![];
    for case in r6_cases().into_iter().filter(|c| c.name == "U+0020/mode0" || c.name == "U+00A0/mode0" || c.name.starts_with("encoded-") || c.name.starts_with("JSON-")) {
        record_sync_failure(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, &case.url, false, &case.secrets).unwrap();
        let summary: String = f.book.connection().query_row("SELECT last_sync_summary FROM bankfeeds_connection WHERE id=?1", [&f.conn], |r| r.get(0)).unwrap();
        let audit: String =
            f.book.connection().query_row("SELECT detail FROM ledger_auditevent WHERE action='bankfeed.synced' ORDER BY occurred_at DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        if r6_leaks(&summary) {
            failures.push(format!("{}/stored-summary", case.name));
        }
        if r6_leaks(&audit) {
            failures.push(format!("{}/stored-audit", case.name));
        }
    }
    f.reopen_clean();
    r5_finish(&mut f);
    assert!(failures.is_empty(), "credential persisted in {} summaries/audits: {failures:?}", failures.len());
}

#[test]
fn r6_accept_sql_fail_after_every_kind_has_no_partial_writes() {
    let mut reached = 0;
    for kind in ["JOURNAL", "CHECK", "DEPOSIT", "TRANSFER", "MATCH"] {
        for event in ["bankfeed.accepted", "bankfeed.matched"] {
            let (mut f, remote, input) = fix1_accept_case(kind);
            if kind == "CHECK" {
                r5_seed_check(&mut f, remote, &input);
            }
            let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
            f.book
                .connection()
                .execute_batch(&format!("CREATE TEMP TRIGGER r6_fail AFTER INSERT ON ledger_auditevent WHEN NEW.action='{event}' BEGIN SELECT RAISE(FAIL,'r6_failure'); END;"))
                .unwrap();
            let result = f.accept(remote, input.clone());
            f.book.connection().execute_batch("DROP TRIGGER temp.r6_fail").unwrap();
            if let Err(e) = result {
                assert!(e.to_string().contains("r6_failure"));
                assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(), before, "{kind}/{event}");
                f.reopen_clean();
                f.accept(remote, input).unwrap();
                reached += 1;
            }
            f.reopen_clean();
            r5_finish(&mut f);
        }
    }
    assert_eq!(reached, 5);
}

#[test]
fn r6_failed_first_pull_does_not_leave_identity_secret_or_downloaded_rows() {
    let mut f = Fx::new();
    let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    f.book
        .connection()
        .execute_batch("CREATE TEMP TRIGGER r6_syncfail AFTER INSERT ON ledger_auditevent WHEN NEW.action='bankfeed.synced' BEGIN SELECT RAISE(FAIL,'r6_syncfailure'); END;")
        .unwrap();
    let e = store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &demo(), NOW, false, None, &SecretSet::none()).unwrap_err();
    assert!(e.to_string().contains("r6_syncfailure"));
    f.book.connection().execute_batch("DROP TRIGGER temp.r6_syncfail").unwrap();
    assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(), before);
    f.reopen_clean();
    assert_eq!(f.sync(&demo())["new"], 10);
    r5_finish(&mut f);
}

/// Identity is computed from the raw value before masking (FIX5 kept): two ids that differ only in a
/// KNOWN secret stay two rows, though both display the same masked text.
#[test]
fn fix6_identity_comes_from_raw_ids_before_masking() {
    let mut f = Fx::new();
    let s = SecretSet::masking_from_credentials("twin-user-one", "twin-pass-one", ["twin-user-two"]).unwrap();
    let (first, second) = ("https://twin-user-one:twin-pass-one@r6.test/account", "https://twin-user-two:pw@r6.test/account");
    let pull = StagedPull {
        accounts: vec![acct(first, "Twin A", vec![tx(&format!("{first}/tx"), "2026-10-20", 100, "A")]), acct(second, "Twin B", vec![tx(&format!("{second}/tx"), "2026-10-20", 100, "B")])],
        ..Default::default()
    };
    let v = store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &pull, NOW, false, None, &s).unwrap();
    assert_eq!((v["accounts"].as_i64(), v["new"].as_i64()), (Some(2), Some(2)));
    let shown: Vec<String> = f.book.connection().prepare("SELECT remote_id FROM bankfeeds_account ORDER BY name").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(shown, ["https://***@r6.test/account", "https://***@r6.test/account"]);
    assert_ne!(f.feed(first), f.feed(second));
    assert_ne!(f.item(&format!("{first}/tx")).id, f.item(&format!("{second}/tx")).id);
    let again = store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &pull, NOW, false, None, &s).unwrap();
    assert_eq!((again["new"].as_i64(), again["unchanged"].as_i64()), (Some(0), Some(2)));
    r5_finish(&mut f);
}

#[test]
fn r6_identity_twins_reopen_after_sequential_service_edits() {
    let mut edits = 0;
    for decision in ["JOURNAL", "CHECK", "DEPOSIT", "TRANSFER", "MATCH"] {
        let (mut f, remote, input) = fix1_accept_case(decision);
        let item = f.accept(remote, input).unwrap();
        let first = "https://twinfirst:pw@r6.test/account";
        let second = "https://twinsecond:pw@r6.test/account";
        let mut pull = StagedPull { accounts: vec![acct(first, "Twin A", vec![]), acct(second, "Twin B", vec![])], ..Default::default() };
        f.sync(&pull);
        f.reopen_clean();
        edits += 1;
        let spare = services::save_account(&mut f.book, f.u, f.c, AccountInput::new("151", "Twin unused").kind("ASSET")).unwrap().id;
        let first_id = f.feed(first);
        map_account(&mut f.book, f.u, f.c, &first_id, &json!(spare.to_db()), &SecretSet::none()).unwrap();
        f.reopen_clean();
        edits += 1;
        for kind in ["INCOME", "LIABILITY", "EXPENSE", "ASSET", "EQUITY"] {
            services::save_account(&mut f.book, f.u, f.c, AccountInput::new("151", "Reclassified twin").kind(kind).edit(spare)).unwrap();
            f.reopen_clean();
            edits += 1;
        }
        for (active, postable) in [(false, true), (true, false), (true, true)] {
            services::save_account(&mut f.book, f.u, f.c, AccountInput::new("151", "Historical twin").kind("ASSET").active(active).postable(postable).edit(spare)).unwrap();
            f.reopen_clean();
            edits += 1;
        }
        f.map(first, None).unwrap();
        f.reopen_clean();
        edits += 1;
        let second_id = f.feed(second);
        map_account(&mut f.book, f.u, f.c, &second_id, &json!(spare.to_db()), &SecretSet::none()).unwrap();
        f.reopen_clean();
        edits += 1;
        for currency in ["EUR", "USD"] {
            if services::edit_company(&mut f.book, f.u, f.c, "Changed twins", currency).is_ok() {
                edits += 1;
            }
            f.reopen_clean();
        }
        for a in &mut pull.accounts {
            a.currency = "EUR".into();
            a.name = "Changed provider twin".into();
        }
        f.sync(&pull);
        f.reopen_clean();
        edits += 1;
        users::add_member(&mut f.book, f.c, f.outsider).unwrap();
        f.reopen_clean();
        edits += 1;
        users::set_active(&mut f.book, f.u, false).unwrap();
        f.reopen_clean();
        edits += 1;
        users::set_active(&mut f.book, f.u, true).unwrap();
        f.reopen_clean();
        edits += 1;
        let _ = return_to_review(&mut f.book, f.u, f.c, &item.id, &SecretSet::none());
        f.reopen_clean();
        forget(&mut f.book, f.u, f.c, &f.conn.clone(), &SecretSet::none()).unwrap();
        f.reopen_clean();
        edits += 1;
        r5_finish(&mut f);
    }
    eprintln!("R6 {edits} independent sequential edits reopen with credential-identity twins across five acceptance kinds");
}
