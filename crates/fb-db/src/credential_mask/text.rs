//! `bankfeeds/client.py` `redact` / `sanitize`, `services.default_check_number` and `rule_for`'s
//! `str.lower()`: the only text rules the core half calls. The provider half lives in the simplefin
//! plugin; core keeps its own copy because core never depends on a plugin crate.
//!
//! Python's `re` decides `\b`, `\s`, `\d` and IGNORECASE with its own Unicode 15.1 tables (a word
//! character is `str.isalnum()` or `_`; `(?i)i` also matches `İ` and `ı`), which differ from the `regex`
//! crate's (combining marks, joiners and connector punctuation are word characters there). The patterns are
//! simple enough to scan by hand against tables generated from the frozen interpreter (`py_unicode.rs`,
//! `fb-harness/py/gen_bankfeeds_unicode.py`), with differential vectors from the frozen functions in
//! `testdata/python-*.json`.
use super::py_unicode as table;
use regex::Regex;
use std::sync::LazyLock;

static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("static regex"));

fn within(c: char, ranges: &[(u32, u32)]) -> bool {
    let c = c as u32;
    ranges
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}
/// Python `re` `\w` (str pattern).
pub fn is_word(c: char) -> bool {
    within(c, table::WORD)
}
/// Python `re` `\s` (str pattern).
pub fn is_space(c: char) -> bool {
    within(c, table::SPACE)
}
/// Python `re` `\d` (str pattern).
pub fn is_digit(c: char) -> bool {
    within(c, table::DIGIT)
}
/// Python `re` `\b` before position `i` when the character at `i` is a word character.
fn word_start(s: &[char], i: usize) -> bool {
    i == 0 || !is_word(s[i - 1])
}

/// Python 3.13 `str.lower()`: full lowercase mapping plus the final-sigma context rule.
pub fn py_lower(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '\u{3a3}' {
            out.push(if final_sigma(&chars, i) { '\u{3c2}' } else { '\u{3c3}' });
            continue;
        }
        match table::LOWER.binary_search_by_key(&(c as u32), |&(k, _)| k) {
            Ok(at) => out.push_str(table::LOWER[at].1),
            Err(_) => out.push(c),
        }
    }
    out
}
/// `handle_capital_sigma`: preceded by a cased letter and not followed by one, case-ignorables skipped.
fn final_sigma(s: &[char], i: usize) -> bool {
    let ignorable = |c: char| within(c, table::CASE_IGNORABLE);
    let cased = |c: char| within(c, table::CASED);
    let before = s[..i].iter().rev().find(|&&c| !ignorable(c));
    let after = s[i + 1..].iter().find(|&&c| !ignorable(c));
    before.is_some_and(|&c| cased(c)) && !after.is_some_and(|&c| cased(c))
}

/// One pattern character under Python's IGNORECASE (ASCII letters plus the folds `re` adds).
fn ci(c: char, want: char) -> bool {
    c.eq_ignore_ascii_case(&want)
        || match want {
            's' => c == '\u{17f}',
            'i' => c == '\u{130}' || c == '\u{131}',
            'k' => c == '\u{212a}',
            _ => false,
        }
}
fn ci_word(s: &[char], i: usize, word: &str) -> bool {
    let n = word.chars().count();
    i + n <= s.len() && s[i..i + n].iter().zip(word.chars()).all(|(&c, w)| ci(c, w))
}
/// `[A-Za-z]` under IGNORECASE.
fn ci_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '\u{130}' | '\u{131}' | '\u{17f}' | '\u{212a}')
}
fn skip_spaces(s: &[char], mut i: usize) -> usize {
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    i
}

/// `_URL = (?i)\bhttps?://[^\s<>"']+`: the match length at `i`.
fn url_at(s: &[char], i: usize) -> Option<usize> {
    if !word_start(s, i) || !ci_word(s, i, "http") {
        return None;
    }
    let mut j = i + 4;
    if j < s.len() && ci(s[j], 's') {
        j += 1;
    }
    if !(j + 3 <= s.len() && s[j] == ':' && s[j + 1] == '/' && s[j + 2] == '/') {
        return None;
    }
    let start = j + 3;
    let mut end = start;
    while end < s.len() && !is_space(s[end]) && !matches!(s[end], '<' | '>' | '"' | '\'') {
        end += 1;
    }
    (end > start).then_some(end - i)
}
/// `_BASIC_HEADER = (?i)(authorization\s*[:=]\s*basic\s+)[A-Za-z0-9+/=._~-]+`: (prefix, total) at `i`.
/// `glue[k]` is true where hidden characters were folded out before `s[k]` ([`Folded`]); a fold
/// separates `basic` from its token like `\s+` does.
fn basic_header_at(s: &[char], glue: &[bool], i: usize) -> Option<(usize, usize)> {
    if !ci_word(s, i, "authorization") {
        return None;
    }
    let j = skip_spaces(s, i + 13);
    if !(j < s.len() && matches!(s[j], ':' | '=')) {
        return None;
    }
    let j = skip_spaces(s, j + 1);
    if !ci_word(s, j, "basic") {
        return None;
    }
    let prefix = skip_spaces(s, j + 5);
    if prefix == j + 5 && !glue[prefix] {
        return None;
    }
    let token = |c: char| ci_letter(c) || c.is_ascii_digit() || "+/=._~-".contains(c);
    let end = (prefix..s.len()).find(|&k| !token(s[k])).unwrap_or(s.len());
    (end > prefix).then_some((prefix - i, end - i))
}
/// `_BASIC = (?i)\b(basic\s+)(?=[A-Za-z0-9+/]*[0-9+/=])[A-Za-z0-9+/]{8,}={0,2}`: (prefix, total) at `i`.
/// A fold ([`Folded`]) counts as `\b` before `basic` and as `\s+` after it.
fn basic_at(s: &[char], glue: &[bool], i: usize) -> Option<(usize, usize)> {
    if !(word_start(s, i) || glue[i]) || !ci_word(s, i, "basic") {
        return None;
    }
    let prefix = skip_spaces(s, i + 5);
    if prefix == i + 5 && !glue[prefix] {
        return None;
    }
    let class = |c: char| ci_letter(c) || c.is_ascii_digit() || c == '+' || c == '/';
    let run = (prefix..s.len()).find(|&k| !class(s[k])).unwrap_or(s.len());
    let lookahead = s[prefix..run].iter().any(|&c| c.is_ascii_digit() || c == '+' || c == '/') || s.get(run) == Some(&'=');
    if !lookahead || run - prefix < 8 {
        return None;
    }
    let mut end = run;
    while end < s.len() && end < run + 2 && s[end] == '=' {
        end += 1;
    }
    Some((prefix - i, end - i))
}
/// `re.sub` with a hand-written matcher: left to right, a match resumes the scan at its end.
fn substitute(s: &[char], at: impl Fn(&[char], usize) -> Option<(usize, usize)>, replace: impl Fn(&[char], usize) -> String) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if let Some((prefix, total)) = at(s, i) {
            out.extend(replace(&s[i..i + total], prefix).chars());
            i += total;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// Not copied from Python (fuzz audit F-04b): characters that print as nothing, i.e. the C0 controls
/// that are not `\s` (NUL, BEL, BS, ...), DEL, and the zero-width space, joiners, word joiner and BOM.
/// Python runs its credential patterns over them and blanks the controls only afterwards, so
/// `Basic\0 <token>`, `Auth\0orization: Basic <token>` or a token split by `\u{200b}` keeps the token.
pub fn is_hidden(c: char) -> bool {
    (c <= '\u{1f}' && !is_space(c)) || c == '\u{7f}' || matches!(c, '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}')
}

/// Text with its hidden characters ([`is_hidden`]) folded out for the credential matchers:
/// `hidden[k]` holds the ones that stood before `chars[k]` (`hidden[chars.len()]` the trailing ones).
/// Keywords and tokens match across a fold, and a fold also counts as the separator after `basic`
/// (and as `\b` before it), so a credential is caught however hidden characters split it. Everything
/// outside a redaction unfolds to exactly the input; hidden characters inside a redacted token go.
struct Folded {
    chars: Vec<char>,
    hidden: Vec<Vec<char>>,
}

fn fold(s: &[char]) -> Folded {
    let mut out = Folded { chars: Vec::with_capacity(s.len()), hidden: vec![vec![]] };
    for &c in s {
        if is_hidden(c) {
            out.hidden.last_mut().expect("one slot per char plus one").push(c);
        } else {
            out.chars.push(c);
            out.hidden.push(vec![]);
        }
    }
    out
}

fn unfold(f: &Folded) -> Vec<char> {
    let mut out = Vec::with_capacity(f.chars.len());
    for (k, &c) in f.chars.iter().enumerate() {
        out.extend_from_slice(&f.hidden[k]);
        out.push(c);
    }
    out.extend_from_slice(&f.hidden[f.chars.len()]);
    out
}

/// `re.sub(pattern, r'\1***', text)` over folded text: a match keeps its prefix (and the hidden
/// characters in and right after it) and replaces the token, hidden characters included, by `***`.
fn redact_folded(f: &Folded, at: impl Fn(&[char], &[bool], usize) -> Option<(usize, usize)>) -> Folded {
    let glue: Vec<bool> = f.hidden.iter().map(|h| !h.is_empty()).collect();
    let mut out = Folded { chars: Vec::with_capacity(f.chars.len()), hidden: vec![vec![]] };
    let mut i = 0;
    while i < f.chars.len() {
        out.hidden.last_mut().expect("slot").extend_from_slice(&f.hidden[i]);
        if let Some((prefix, total)) = at(&f.chars, &glue, i) {
            for k in i..i + prefix {
                if k > i {
                    out.hidden.last_mut().expect("slot").extend_from_slice(&f.hidden[k]);
                }
                out.chars.push(f.chars[k]);
                out.hidden.push(vec![]);
            }
            out.hidden.last_mut().expect("slot").extend_from_slice(&f.hidden[i + prefix]);
            for c in "***".chars() {
                out.chars.push(c);
                out.hidden.push(vec![]);
            }
            i += total;
        } else {
            out.chars.push(f.chars[i]);
            out.hidden.push(vec![]);
            i += 1;
        }
    }
    out.hidden.last_mut().expect("slot").extend_from_slice(&f.hidden[f.chars.len()]);
    out
}

/// Python `ipaddress.ip_address` for a bracketed host: IPv4 (refused in brackets) or IPv6 with scope.
fn ipv4(s: &str) -> bool {
    let octets: Vec<&str> = s.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|o| {
            !o.is_empty() && o.len() <= 3 && o.bytes().all(|b| b.is_ascii_digit()) && (*o == "0" || !o.starts_with('0')) && o.parse::<u16>().is_ok_and(|v| v <= 255)
        })
}
fn ipv6(s: &str) -> bool {
    let (addr, scope) = match s.split_once('%') {
        Some((a, scope)) => (a, Some(scope)),
        None => (s, None),
    };
    if scope.is_some_and(|scope| scope.is_empty() || scope.contains('%')) || addr.is_empty() {
        return false;
    }
    let mut parts: Vec<String> = addr.split(':').map(str::to_owned).collect();
    if parts.len() < 3 {
        return false;
    }
    if parts.last().is_some_and(|p| p.contains('.')) {
        let tail = parts.pop().unwrap_or_default();
        if !ipv4(&tail) {
            return false;
        }
        parts.push("0".into());
        parts.push("0".into());
    }
    if parts.len() > 9 {
        return false;
    }
    let mut skip = None;
    for (i, part) in parts.iter().enumerate().take(parts.len() - 1).skip(1) {
        if part.is_empty() {
            if skip.is_some() {
                return false;
            }
            skip = Some(i);
        }
    }
    let n = parts.len();
    let (hi, lo) = match skip {
        Some(at) => {
            let (mut hi, mut lo) = (at, n - at - 1);
            if parts[0].is_empty() {
                hi -= 1;
                if hi != 0 {
                    return false;
                }
            }
            if parts[n - 1].is_empty() {
                lo -= 1;
                if lo != 0 {
                    return false;
                }
            }
            if hi + lo >= 8 {
                return false;
            }
            (hi, lo)
        }
        None => {
            if n != 8 {
                return false;
            }
            (n, 0)
        }
    };
    let hextet = |h: &String| !h.is_empty() && h.len() <= 4 && h.bytes().all(|b| b.is_ascii_hexdigit());
    parts[..hi].iter().all(hextet) && parts[n - lo..].iter().all(hextet)
}
fn bracketed_host_ok(host: &str) -> bool {
    if let Some(rest) = host.strip_prefix('v') {
        // `\Av[a-fA-F0-9]+\..+\Z`
        let hex = rest.chars().take_while(char::is_ascii_hexdigit).count();
        return hex > 0 && rest[hex..].starts_with('.') && rest[hex + 1..].chars().any(|c| c != '\n') && !rest[hex + 1..].contains('\n');
    }
    !ipv4(host) && ipv6(host)
}
/// `_check_bracketed_netloc`.
fn bracketed_netloc_ok(netloc: &str) -> bool {
    let host_port = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);
    let host = match host_port.split_once('[') {
        Some((before, bracketed)) => {
            if !before.is_empty() {
                return false;
            }
            let (host, port) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            if !port.is_empty() && !port.starts_with(':') {
                return false;
            }
            host
        }
        None => host_port.split_once(':').map_or(host_port, |(h, _)| h),
    };
    bracketed_host_ok(host)
}
struct Split {
    scheme: String,
    netloc: String,
    path: String,
    query: String,
    fragment: String,
}
/// `urllib.parse.urlsplit` (3.13) on a `_URL` match: no leading controls, spaces, tabs or newlines remain.
fn urlsplit(url: &str) -> Option<Split> {
    let mut scheme = String::new();
    let mut rest = url;
    if let Some(i) = url.find(':')
        && i > 0
        && url.as_bytes()[0].is_ascii_alphabetic()
        && url[..i].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        scheme = url[..i].to_ascii_lowercase();
        rest = &url[i + 1..];
    }
    let mut netloc = "";
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = &after[..end];
        rest = &after[end..];
        if netloc.contains('[') != netloc.contains(']') {
            return None;
        }
        if netloc.contains('[') && !bracketed_netloc_ok(netloc) {
            return None;
        }
    }
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    // `_checknetloc`: a non-ASCII netloc whose NFKC form introduces '/', '?', '#', '@' or ':'.
    if netloc.chars().any(|c| within(c, table::NFKC_DELIMITERS)) {
        return None;
    }
    Some(Split { scheme, netloc: netloc.to_owned(), path: path.to_owned(), query: query.to_owned(), fragment: fragment.to_owned() })
}
/// `urllib.parse.urlunsplit`, called only with a non-empty netloc (`***@host`).
fn urlunsplit(p: &Split) -> String {
    let mut url = p.path.clone();
    if !url.is_empty() && !url.starts_with('/') {
        url.insert(0, '/');
    }
    url = format!("//{}{url}", p.netloc);
    if !p.scheme.is_empty() {
        url = format!("{}:{url}", p.scheme);
    }
    if !p.query.is_empty() {
        url = format!("{url}?{}", p.query);
    }
    if !p.fragment.is_empty() {
        url = format!("{url}#{}", p.fragment);
    }
    url
}
/// `_strip_userinfo`.
fn strip_userinfo(url: &str) -> String {
    let Some(mut parts) = urlsplit(url) else {
        return format!("{}://***", url.split_once("://").map_or(url, |(scheme, _)| scheme));
    };
    let mut url = url.to_owned();
    if let Some((_, host)) = parts.netloc.rsplit_once('@') {
        parts.netloc = format!("***@{host}");
        url = urlunsplit(&parts);
    }
    let (scheme, rest) = url.split_once("://").unwrap_or((&url, ""));
    if rest.replacen("***@", "", 1).contains('@') {
        let host = rest.rsplit_once('@').map_or("", |(_, h)| h);
        return format!("{scheme}://***@{host}");
    }
    url
}

/// Not copied from Python (L4-BANKFEEDS.md "Python bugs not copied"): `_URL` needs `\b` before `http`, so
/// Python keeps the credentials of a URL glued to a word character (`xhttps://u:p@h`, also after tag
/// removal), and it never looks at other schemes. After Python's passes, every `://` whose run (up to
/// whitespace or `<>"'`, as `_URL`) still holds an `@` loses everything before that run's last `@`,
/// exactly as `_strip_userinfo` collapses a URL with `@` beyond its netloc. Python's own output
/// (`***@host`) is left unchanged, so this differs from Python only where Python keeps a credential.
fn strip_any_userinfo(s: &[char]) -> Vec<char> {
    let stop = |c: char| is_space(c) || matches!(c, '<' | '>' | '"' | '\'');
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == ':' && s.get(i + 1) == Some(&'/') && s.get(i + 2) == Some(&'/') {
            let start = i + 3;
            let end = (start..s.len()).find(|&k| stop(s[k])).unwrap_or(s.len());
            if let Some(at) = s[start..end].iter().rposition(|&c| c == '@')
                && s[start..start + at] != ['*', '*', '*']
            {
                out.extend("://***".chars());
                i = start + at;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// Spellings of the URL delimiters the structural pass treats alike: ASCII, percent-encoded (any case,
/// `@` also double-encoded) and every code point whose NFKC form is the delimiter (Python 3.13
/// `unicodedata`: `staging/l4-fix3/nfkc.py`). `\` counts as a slash, as browsers read `https:\\host`.
const COLONS: &[&str] = &[":", "%3a", "\u{fe13}", "\u{fe55}", "\u{ff1a}"];
const SLASHES: &[&str] = &["/", "\\", "%2f", "%5c", "\u{ff0f}"];
const ATS: &[&str] = &["@", "%40", "%2540", "\u{fe6b}", "\u{ff20}"];
/// The length of the first spelling in `from` found at `s[i..]` (ASCII letters in any case).
fn spelled_at(s: &[char], i: usize, from: &[&str]) -> Option<usize> {
    from.iter().find_map(|w| {
        let n = w.chars().count();
        (i + n <= s.len() && s[i..i + n].iter().zip(w.chars()).all(|(c, w)| c.eq_ignore_ascii_case(&w))).then_some(n)
    })
}
/// `scheme://` in any spelling at `i`: the separator's length.
fn separator_at(s: &[char], i: usize) -> Option<usize> {
    let colon = spelled_at(s, i, COLONS)?;
    let first = spelled_at(s, i + colon, SLASHES)?;
    let second = spelled_at(s, i + colon + first, SLASHES)?;
    Some(colon + first + second)
}

/// Not copied from Python (L4-BANKFEEDS.md "Python bugs not copied", FIX3): the structural userinfo rule.
/// After every `scheme://` (any scheme, any case, whatever precedes it, in any spelling of
/// [`separator_at`]) the authority runs to the next literal `/`, `?`, `#` or whitespace (or the end), and
/// everything before its last `@` (any spelling in [`ATS`]) is userinfo and becomes `***`. No character
/// class decides where userinfo ends: quotes, `<>`, `;`, `%`, controls and non-ASCII all stay inside it.
/// Userinfo that is already `***` (Python's output) is left as it is.
fn strip_authority_userinfo(s: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if let Some(sep) = separator_at(s, i) {
            let start = i + sep;
            let end = (start..s.len()).find(|&k| matches!(s[k], '/' | '?' | '#') || is_space(s[k])).unwrap_or(s.len());
            let at = (start..end).rev().find(|&k| spelled_at(s, k, ATS).is_some_and(|n| k + n <= end));
            out.extend_from_slice(&s[i..start]);
            match at {
                Some(at) if at > start && s[start..at] != ['*', '*', '*'] => {
                    out.extend("***".chars());
                    i = at;
                }
                _ => i = start,
            }
            continue;
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// v2's passes after Python's: the run rule ([`strip_any_userinfo`], Python's collapse for `@` beyond
/// the netloc) and the structural authority rule ([`strip_authority_userinfo`]), repeated until neither
/// changes anything: one pass's `***@` can complete a run the other pass then collapses
/// (`https://x://'u:p@h` → `https://x://***@h` → `https://***@h`). Every change removes userinfo that is
/// not `***`, and `***` is never removed again, so the loop ends; the bound is only a backstop.
fn strip_all_userinfo(s: &[char]) -> Vec<char> {
    let mut current = s.to_vec();
    for _ in 0..=s.len() {
        let next = strip_authority_userinfo(&strip_any_userinfo(&current));
        if next == current {
            break;
        }
        current = next;
    }
    current
}

/// `redact`: URL credentials, HTTP Basic credentials and control characters; then, stricter than Python,
/// the userinfo of every remaining `scheme://…@` ([`strip_all_userinfo`]). Also stricter than Python
/// (fuzz audit F-04b): the Basic matchers see the text with hidden characters folded out ([`Folded`]),
/// so a control or zero-width character cannot split a credential past them. Without hidden
/// characters the result is Python's, character for character.
pub fn redact(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let urls = substitute(&chars, |s, i| url_at(s, i).map(|n| (0, n)), |m, _| strip_userinfo(&m.iter().collect::<String>()));
    let folded = fold(&urls);
    let basic = unfold(&redact_folded(&redact_folded(&folded, basic_header_at), basic_at));
    strip_all_userinfo(&basic).into_iter().map(|c| if c <= '\u{1f}' || c == '\u{7f}' { ' ' } else { c }).collect()
}

/// v2's stricter passes alone ([`strip_all_userinfo`]): the harness applies them to the Python reference,
/// whose `redact` keeps these, and the accept / ignore audits apply them to the bank description.
pub fn strip_url_userinfo(text: &str) -> String {
    strip_all_userinfo(&text.chars().collect::<Vec<_>>()).into_iter().collect()
}

/// True when `text` still carries URL userinfo, by either rule: some `://` run (up to whitespace or
/// `<>"'`) has an `@` with anything but `***` before it, or some separator in any spelling is followed by
/// an authority (up to `/`, `?`, `#` or whitespace) whose last `@` in any spelling has anything but `***`
/// (or nothing) before it. Written with the `regex` crate, apart from the stripping code, for the tests
/// and the harness to assert credential absence.
pub fn has_url_userinfo(text: &str) -> bool {
    static USERINFO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"://([^\s<>"']*)@"#).expect("static regex"));
    static SEPARATOR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\A(?::|%3a|\x{FE13}|\x{FE55}|\x{FF1A})(?:/|\\|%2f|%5c|\x{FF0F}){2}").expect("static regex"));
    static AUTHORITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[^/?#\s\x1c-\x1f]*").expect("static regex"));
    static AT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\A(?:@|%40|%2540|\x{FE6B}|\x{FF20})").expect("static regex"));
    // `[^\s]` is Unicode White_Space; Python's `\s` adds U+001C..U+001F, which redact has already blanked.
    if USERINFO.captures_iter(text).any(|c| &c[1] != "***") {
        return true;
    }
    // Every separator start, overlapping ones included (`:///` holds two), so none hides behind another.
    let starts = |t: &str| (0..t.len()).filter(|&i| t.is_char_boundary(i)).collect::<Vec<_>>();
    starts(text).into_iter().filter_map(|i| SEPARATOR.find(&text[i..]).map(|m| i + m.end())).any(|after| {
        let authority = AUTHORITY.find(&text[after..]).map_or("", |a| a.as_str());
        // The last `@` spelling: the latest start of any spelling, overlapping ones included.
        let last = starts(authority).into_iter().rev().find(|&k| AT.is_match(&authority[k..]));
        last.is_some_and(|k| !authority[..k].is_empty() && &authority[..k] != "***")
    })
}

/// `sanitize(text, limit)`: tags stripped, redacted, Python-stripped and cut to `limit` code points.
///
/// Not copied from Python (FIX3): Python removes tags before it redacts, so a `<` inside userinfo and a
/// `>` after its `@` delete the `@` and leave the start of the userinfo behind (`https://user<x:pass@h>/p`
/// → `https://user/p`). A tag that holds no `@` cannot do that: the authority's last `@` survives and the
/// rules after tag removal still take everything before it. So only when some tag holds an `@` (any
/// spelling) does v2 first remove the userinfo as written ([`strip_url_userinfo`]), then follow Python.
pub fn sanitize(text: &str, limit: usize) -> String {
    let at_in_tag = TAGS.find_iter(text).any(|m| {
        let tag: Vec<char> = m.as_str().chars().collect();
        (0..tag.len()).any(|k| spelled_at(&tag, k, ATS).is_some())
    });
    let text = if at_in_tag { strip_url_userinfo(text) } else { text.to_owned() };
    redact(&TAGS.replace_all(&text, "")).trim_matches(fb_money::py_isspace).chars().take(limit).collect()
}

/// `re.search(r'\bCHECK\s*#?\s*(\d{1,18})\b', description, re.IGNORECASE)` group 1.
pub fn check_number_in(description: &str) -> Option<String> {
    let s: Vec<char> = description.chars().collect();
    (0..s.len()).find_map(|i| {
        if !word_start(&s, i) || !ci_word(&s, i, "check") {
            return None;
        }
        let mut j = skip_spaces(&s, i + 5);
        if s.get(j) == Some(&'#') {
            j = skip_spaces(&s, j + 1);
        }
        let end = (j..s.len()).find(|&k| !is_digit(s[k])).unwrap_or(s.len());
        let ok = (1..=18).contains(&(end - j)) && !s.get(end).is_some_and(|&c| is_word(c));
        ok.then(|| s[j..end].iter().collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sanitize_strips_tags_credentials_and_controls() {
        assert_eq!(sanitize("  <b>PAY</b>\tROLL\n ", 300), "PAY ROLL");
        assert_eq!(sanitize("see https://user:pa@ss@bridge.example/x now", 300), "see https://***@bridge.example/x now");
        assert_eq!(sanitize("HTTPS://u:p@h.example/a?", 300), "https://***@h.example/a");
        assert_eq!(sanitize("Authorization: Basic dXNlcjpwYXNz", 300), "Authorization: Basic ***");
        assert_eq!(sanitize("basic ABCDEFGHIJ", 300), "basic ABCDEFGHIJ");
        assert_eq!(sanitize("basic ABCDEFGH1J", 300), "basic ***");
        assert_eq!(sanitize("ééééé", 3), "ééé");
        assert_eq!(sanitize("https://[bad@x/", 300), "https://***");
    }
    fn vectors(raw: &str) -> Vec<Vec<String>> {
        serde_json::from_str(raw).expect("vectors")
    }
    /// True when `rust` is `python` with some spans (possibly none) replaced by `***`: v2 only redacted more.
    fn only_redacted_more(python: &str, rust: &str) -> bool {
        let pattern = rust.split("***").map(regex::escape).collect::<Vec<_>>().join("(?s:.*?)");
        Regex::new(&format!("^{pattern}$")).expect("escaped pattern").is_match(python)
    }
    /// Parity with the frozen functions wherever Python removes every credential; where the outputs differ,
    /// Python's must still carry URL userinfo and v2's must not (L4-BANKFEEDS.md "Python bugs not copied"),
    /// or (fuzz audit F-04b) the input holds hidden characters ([`is_hidden`]) and v2 only redacted more.
    /// v2's output never carries userinfo, on any vector.
    #[test]
    fn redact_and_sanitize_equal_frozen_python_except_kept_credentials() {
        let (mut failures, mut stricter, mut folded) = (vec![], 0, 0);
        for v in vectors(include_str!("testdata/python-redact.json")) {
            let (rust, clean) = (redact(&v[0]), sanitize(&v[0], 120));
            for (python, rust) in [(&v[1], &rust), (&v[2], &clean)] {
                if has_url_userinfo(rust) {
                    failures.push(format!("{:?}: rust kept userinfo: {rust:?}", v[0]));
                } else if python != rust {
                    if has_url_userinfo(python) {
                        stricter += 1;
                    } else if v[0].chars().any(is_hidden) && only_redacted_more(python, rust) {
                        folded += 1;
                    } else {
                        failures.push(format!("{:?}: python {python:?}, rust {rust:?}", v[0]));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{} differences, first: {:#?}", failures.len(), &failures[..failures.len().min(8)]);
        // The FIX1 run counted 27 inputs where Python keeps glued credentials (redact and sanitize each).
        assert!(stricter >= 27, "expected the known Python credential leaks among the vectors, saw {stricter}");
        // F-04b: 15 outputs (redact and sanitize) where hidden characters split a Basic credential for Python.
        assert!(folded <= 40, "hidden-character differences grew to {folded}; look at them");
    }
    #[test]
    fn glued_and_other_scheme_credentials_are_removed() {
        for (input, want) in [
            ("xhttps://user:pass@host/path", "xhttps://***@host/path"),
            ("_https://user:pass@host/path", "_https://***@host/path"),
            ("9https://user:pass@host/path", "9https://***@host/path"),
            ("éhttps://user:pass@host/path", "éhttps://***@host/path"),
            ("failed<b></b>https://user:pass@host/path now", "failedhttps://***@host/path now"),
            ("ftp://user:pass@files.example/x", "ftp://***@files.example/x"),
            ("xhttps://token@host", "xhttps://***@host"),
            ("xhttps://us/er:pass@host/p", "xhttps://***@host/p"),
            ("xhttps://a\u{1}b:pass@host", "xhttps://***@host"),
            ("x://a@b://c@d e@f", "x://***@d e@f"),
            ("see https://user:pass@host/x now", "see https://***@host/x now"),
            ("mail bob@example.com or https://host/x", "mail bob@example.com or https://host/x"),
        ] {
            assert_eq!(sanitize(input, 300), want, "{input:?}");
            assert!(!sanitize(input, 300).contains("pass") && !sanitize(input, 300).contains("token"), "{input:?}");
        }
    }
    /// FIX3: the structural rule. Userinfo runs to the authority's last `@` whatever it contains; the
    /// authority ends only at `/`, `?`, `#` or whitespace; separators and `@` count in every spelling.
    #[test]
    fn fix3_structural_userinfo_rule() {
        for (input, want) in [
            ("https://u:pa'ss@host/path", "https://***@host/path"),
            ("see https://r3us'er:r3pass@bridge.example.test/path now", "see https://***@bridge.example.test/path now"),
            ("https://u:pa\"ss@host/p", "https://***@host/p"),
            ("https://u:p<a>ss@host/p", "https://***@host/p"),
            ("https://u<x:pass@h>/p", "https://***@h>/p"),
            ("https://u:pa!$&()*+,;=ss@host", "https://***@host"),
            ("{\"url\":\"https://u:p'a@host\",\"n\":1}", "{\"url\":\"https://***@host\",\"n\":1}"),
            ("https%3A%2F%2Fuser%3Apass%40host%2Fx", "https%3A%2F%2F***%40host%2Fx"),
            ("https:%2f%2fuser:pass@host", "https:%2f%2f***@host"),
            ("https\u{ff1a}\u{ff0f}\u{ff0f}user:pass\u{ff20}host/x", "https\u{ff1a}\u{ff0f}\u{ff0f}***\u{ff20}host/x"),
            ("https:\\\\user:pass@host", "https:\\\\***@host"),
            ("XHTTPS://user:pass%2540host", "XHTTPS://***%2540host"),
            ("https://host/a@b c@d", "https://***@b c@d"),
            ("https://host?x=a@b", "https://***@b"),
            ("https://@host/x", "https://***@host/x"),
            ("ftp://@host/x", "ftp://***@host/x"),
            ("mail bob@example.com or https://host/x", "mail bob@example.com or https://host/x"),
        ] {
            assert_eq!(sanitize(input, 300), want, "{input:?}");
            assert!(!has_url_userinfo(&sanitize(input, 300)), "{input:?}");
        }
        for leaked in ["https://u:pa'ss@host/path", "x://a\"b@c", "https%3A%2F%2Fu%3Ap%40h", "s3\u{ff1a}//u:p\u{fe6b}h", "https://***@x%40y"] {
            assert!(has_url_userinfo(leaked), "{leaked:?}");
        }
    }
    /// Random prefixes and suffixes around credential URLs: the credential never survives `redact` or
    /// `sanitize`, whatever touches the URL (word characters, marks, tags, quotes, controls, schemes).
    #[test]
    fn fuzz_credentials_never_survive_any_prefix_or_suffix() {
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let pieces = [
            "x", "_", "9", "é", "\u{301}", "ß", "İ", "<b>", "</b>", "<i></i>", " ", "\t", "\n", "\u{1}", "\u{1c}", "\u{a0}", "'", "\"", ">", "<",
            "(", ")", ".", ",", ":", "/", "//", "@", "#", "?", "https", "http", "HTTPS", "ftp", "s3", "basic ", "Authorization: Basic ", "😀",
            "中", "\u{200d}", "\u{ff0f}", "[", "]", "%40", "***", "***@", "://",
        ];
        let schemes = ["https", "http", "HTTPS", "hTtP", "ftp", "s3", "git+ssh", "x"];
        let secrets = ["r2user:r2pass", "r2token", "r2user:r2pa@ss", "r2user:", ":r2pass"];
        let hosts = ["bridge.example", "[::1]", "[bad", "h.example:8443", "bridge.example/p@th", ""];
        let mut tried = 0;
        for _ in 0..40_000 {
            let pick = |r: u64, from: &[&'static str]| from[(r % from.len() as u64) as usize];
            let (np, ns) = (next() % 4, next() % 4);
            let prefix: String = (0..np).map(|_| pick(next(), &pieces)).collect();
            let suffix: String = (0..ns).map(|_| pick(next(), &pieces)).collect();
            let (scheme, secret, host) = (pick(next(), &schemes), pick(next(), &secrets), pick(next(), &hosts));
            let input = format!("{prefix}{scheme}://{secret}@{host}{suffix}");
            for out in [redact(&input), sanitize(&input, 500)] {
                assert!(!out.contains("r2user") && !out.contains("r2pass") && !out.contains("r2token"), "{input:?} -> {out:?}");
                assert!(!has_url_userinfo(&out), "{input:?} -> {out:?}");
            }
            tried += 1;
        }
        assert_eq!(tried, 40_000);
    }
    #[test]
    fn check_number_equals_frozen_python() {
        let mut failures = vec![];
        for v in vectors(include_str!("testdata/python-check.json")) {
            let rust = check_number_in(&v[0]).unwrap_or_else(|| "EFT-42".into());
            if rust != v[1] {
                failures.push(format!("{:?}: python {:?}, rust {rust:?}", v[0], v[1]));
            }
        }
        assert!(failures.is_empty(), "{} differences, first: {:#?}", failures.len(), &failures[..failures.len().min(8)]);
    }
    #[test]
    fn lower_equals_frozen_python() {
        let mut failures = vec![];
        for v in vectors(include_str!("testdata/python-lower.json")) {
            if py_lower(&v[0]) != v[1] {
                failures.push(format!("{:?}: python {:?}, rust {:?}", v[0], v[1], py_lower(&v[0])));
            }
        }
        assert!(failures.is_empty(), "{} differences, first: {:#?}", failures.len(), &failures[..failures.len().min(8)]);
    }
}
