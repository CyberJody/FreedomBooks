//! Known-secret masking (FIX6), the way CI systems mask secrets. The structural userinfo rule in
//! [`super::text`] is a best-effort backstop for credential-looking URLs the book never held. It cannot be
//! finished, because every review found another spelling. What the book must never store is its OWN
//! bank-feed credential, and that one is known: the plugin holding the connection's access URL hands the
//! core a [`SecretSet`] built from it, and the core masks every spelling of every value before anything
//! is stored, summarized, audited or returned. The set lives only for the call; the core never persists
//! it (threat model: `L4-BANKFEEDS.md`, "Threat model for credentials").
//!
//! How: each text is decoded once into a lattice. Every position has edges `(character, end)`, one for
//! each way the text from that position can spell a single character (FIX8: a reference that spells two
//! characters gets a node of its own between them, so `&fjlig;` is the path `f`, `j`), escapes nested up to [`DEPTH`]
//! deep: the `%` of a percent escape may itself be `%25` or `%`, the `\` of a JSON escape may be
//! `\\` or `%5C`, the `&` of an entity may be `&amp;`. Every value (and each of its base64 forms) is
//! then matched as a path of characters through the lattice, with gaps allowed between characters. Every
//! run of characters that some match covers becomes [`MASK`]; scanning repeats until nothing matches.
//!
//! Spellings of one character:
//! - raw, and `+` for a space;
//! - percent-encoded UTF-8 bytes (`%XX`), once, twice or more (`%25XX`, `%2525XX`);
//! - JSON: `\uXXXX` (surrogate pairs above U+FFFF), `\/ \" \\ \b \f \n \r \t`, any nesting (`\\u0041`);
//! - HTML character references, read the way `html.unescape` and WHATWG read them: `&#DDD` and `&#xHH`
//!   with any number of leading zeros (the longest digit run; work bounded by the value, which stops
//!   growing past U+10FFFF), `;` optional; `&#128;`..`&#159;` remapped to Windows-1252 (`&#128;` is
//!   `€`), NUL, surrogates and values past U+10FFFF read as U+FFFD, the code points `html.unescape`
//!   drops read as nothing (a gap) as well as themselves; every named reference of HTML5: the
//!   one-character ones (`&amp;`, `&excl;`, `&euro;`, `&NewLine;`…) with `;` optional, and the 93 that
//!   spell two characters (`&fjlig;` is `fj`, `&nvlt;` is `<` and U+20D2) with their `;`; nested
//!   (`&amp;#65;`). Tables: [`super::html_tables`], generated from Python's `html` module;
//! - Rust and Python escapes: `\xHH`, `\u{H…}`, `\UHHHHHHHH`, `\'`;
//! - any mixture of the above, character by character.
//!
//! Gaps: Unicode whitespace (Python's `\s`, Rust's `char::is_whitespace`, plus U+200B..U+200D, U+2060,
//! U+FEFF, U+00AD) may sit between the tokens of an escape; between two characters of a value any run of
//! whitespace in any spelling above (`%20`, `\n`, `\u00a0`, `&#32;`, `+`) and HTML tags (`sanitize`
//! removes tags, which would join the pieces) may sit. Whitespace that is part of the value matches any
//! whitespace, or none.
//!
//! Base64 and base64url, padded or not, are matched at each of the three byte alignments a value can
//! take inside a longer encoded string, by the digits that depend on the value alone; so
//! `Basic base64(user:pass)` is masked by the `user:pass` value. Digits are characters like any other,
//! so a percent- or JSON-escaped `+` or `/` still matches.
//!
//! The secret's own letters match case-sensitively. ASCII case-folding applies to hex digits (and the
//! `x` of an HTML hex entity) only.
//!
//! Short credentials (FIX7): masking a short value everywhere would erase ordinary words from payee
//! names, and leaving it unmasked breaks the guarantee. So no set is built from one: every credential
//! component (user name, password, extra token) needs [`MIN_CREDENTIAL_CHARS`] characters other than
//! whitespace, counted after percent-decoding, or construction fails with [`TOO_SHORT`]. SimpleFIN
//! access URLs used in production are expected to carry long random credentials; short public-demo
//! components remain refused. FIX10 also restricts admitted components to [`CREDENTIAL_ALPHABET`].
//! The full decoder stays for provider encodings and masking-only support of older stored values.
use super::{err, html_tables, text};
use super::MaskResult as FbResult;

/// What a masked secret becomes, the same marker the structural rule writes.
pub const MASK: &str = "***";
/// Shortest credential component a set accepts, in characters other than whitespace.
pub const MIN_CREDENTIAL_CHARS: usize = 12;
/// The refusal when a component is shorter.
pub const TOO_SHORT: &str = "This bank connection's credentials are too short to protect safely; reconnect to get a new access link.";
/// Escape nesting the lattice decodes (`%252541`, `\\\\u0041`, `&amp;amp;#65;`).
pub const DEPTH: usize = 3;
/// A credential alphabet that cannot contain JSON/HTML delimiters or escape introducers.
pub const CREDENTIAL_ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._~-+/=";
pub const UNSAFE_CREDENTIAL: &str = "This bank connection's credentials contain characters FreedomBooks can't store safely. Reconnect to get a new access link.";

fn require_safe(components: &[String]) -> FbResult<()> {
    require_long(components)?;
    if components.iter().any(|v| !v.bytes().all(|b| CREDENTIAL_ALPHABET.as_bytes().contains(&b))) {
        return Err(err(UNSAFE_CREDENTIAL));
    }
    Ok(())
}

/// Characters allowed at each position of one value or base64 form.
type Pattern = Vec<Vec<char>>;

/// The secret values of one connection: built by the plugin (or host) from its stored credential,
/// retained in the open book's writer policy, never persisted or printed (`Debug` shows only a count).
#[derive(Clone, Default)]
pub struct SecretSet {
    values: usize,
    patterns: Vec<Pattern>,
    /// A nonblank first character must consume an edge at the starting lattice node. Index that
    /// necessary condition so a book with many credentials does not try every pattern everywhere.
    starts: std::collections::HashMap<char, Vec<usize>>,
    blank_starts: Vec<usize>,
}
impl std::fmt::Debug for SecretSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretSet({} values)", self.values)
    }
}

impl SecretSet {
    #[cfg(feature = "test-support")]
    pub fn test_masking_new<S: AsRef<str>>(values: impl IntoIterator<Item = S>) -> FbResult<Self> { Self::masking_new(values) }
    #[cfg(feature = "test-support")]
    pub fn test_masking_from_credentials<S: AsRef<str>>(user: &str, password: &str, extra: impl IntoIterator<Item = S>) -> FbResult<Self> { Self::masking_from_credentials(user, password, extra) }
    #[cfg(feature = "test-support")]
    pub fn test_masking_from_access_url<S: AsRef<str>>(url: &str, extra: impl IntoIterator<Item = S>) -> FbResult<Self> { Self::masking_from_access_url(url, extra) }
    /// Validate a typed secret component. Escapes are text here, never decoded.
    pub fn new<S: AsRef<str>>(values: impl IntoIterator<Item = S>) -> FbResult<Self> {
        let values: Vec<String> = values.into_iter().map(|v| v.as_ref().to_owned()).collect();
        require_safe(&values)?;
        Self::masking_new(values)
    }
    pub fn from_credentials<S: AsRef<str>>(user: &str, password: &str, extra: impl IntoIterator<Item = S>) -> FbResult<Self> {
        let extra: Vec<String> = extra.into_iter().map(|v| v.as_ref().to_owned()).collect();
        let mut components = vec![user.to_owned(), password.to_owned()];
        components.extend(extra.iter().cloned());
        require_safe(&components)?;
        Self::masking_from_credentials(user, password, extra)
    }
    /// Validate URL components after URL percent-decoding once. `%25`, for example, is a literal
    /// percent in the credential and is refused; `%2F` is an allowed slash. Extra typed tokens are
    /// validated as supplied. No URL or component is included in an error.
    pub fn from_access_url<S: AsRef<str>>(url: &str, extra: impl IntoIterator<Item = S>) -> FbResult<Self> {
        let mut components: Vec<String> = extra.into_iter().map(|v| v.as_ref().to_owned()).collect();
        let (user, pass) = access_userinfo(url).unwrap_or("").split_once(':').unwrap_or(("", ""));
        components.extend([percent_decode(user).unwrap_or_else(|| user.into()), percent_decode(pass).unwrap_or_else(|| pass.into())]);
        require_safe(&components)?;
        Self::masking_from_access_url(url, components[..components.len()-2].iter())
    }

    /// An opaque in-memory policy identity; never printed or persisted.
    pub(super) fn policy_identity(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        for pattern in &self.patterns {
            hash.update((pattern.len() as u64).to_le_bytes());
            for chars in pattern {
                hash.update((chars.len() as u64).to_le_bytes());
                for c in chars { hash.update(u32::from(*c).to_le_bytes()); }
            }
        }
        format!("bankfeeds:{:x}", hash.finalize())
    }

    /// Combine masking patterns without admitting any new raw credential. The book's policy keeps
    /// one decoder for all loaded connections. The caller deduplicates whole sets first.
    pub(super) fn merge(&mut self, other: &Self) {
        self.values += other.values;
        self.patterns.extend(other.patterns.iter().cloned());
        self.patterns.sort_unstable();
        self.patterns.dedup();
        self.index_starts();
    }

    fn index_starts(&mut self) {
        self.starts.clear();
        self.blank_starts.clear();
        for (i, pattern) in self.patterns.iter().enumerate() {
            if let Some(first) = pattern.first() {
                // Legacy whitespace at the start may match nothing, so it has no required first
                // edge and must always be tried. The matcher remains the authority for every case.
                if first.iter().all(|&c| is_blank(c)) { self.blank_starts.push(i); }
                else { for &c in first { self.starts.entry(c).or_default().push(i); } }
            }
        }
    }

    /// No known secrets: [`SecretSet::mask`] returns its input. For callers that hold no credential
    /// (the harness, tests of the structural backstop).
    pub fn none() -> Self {
        Self::default()
    }
    /// Internal masking-only construction for historical values. Public admission validates the
    /// alphabet before calling this; plugins cannot use this to admit a rejected credential.
    /// The minimum remains necessary to avoid erasing ordinary short words; duplicates are merged.
    pub(super) fn masking_new<S: AsRef<str>>(values: impl IntoIterator<Item = S>) -> FbResult<Self> {
        let values: Vec<String> = values.into_iter().map(|v| v.as_ref().to_owned()).collect();
        require_long(&values)?;
        Ok(Self::build(values))
    }
    /// A user name and password (HTTP Basic, URL userinfo): each alone and `user:pass` together, plus any
    /// other token held in `extra`. Internal masking-only construction; public admission checks
    /// the alphabet separately before using it.
    pub(super) fn masking_from_credentials<S: AsRef<str>>(user: &str, password: &str, extra: impl IntoIterator<Item = S>) -> FbResult<Self> {
        let mut values = vec![user.to_owned(), password.to_owned()];
        values.extend(extra.into_iter().map(|s| s.as_ref().to_owned()));
        require_long(&values)?;
        values.push(format!("{user}:{password}"));
        Ok(Self::build(values))
    }
    /// The secrets of a SimpleFIN access URL (`scheme://user:pass@host/path`): the userinfo, the user name
    /// and the password, each as written and percent-decoded, `user:pass` decoded, and any `extra` token.
    /// Internal masking-only construction for an old stored URL (never returned for use after a
    /// public validation failure). The decoded components must still meet the minimum length.
    pub(super) fn masking_from_access_url<S: AsRef<str>>(url: &str, extra: impl IntoIterator<Item = S>) -> FbResult<Self> {
        let mut values: Vec<String> = extra.into_iter().map(|s| s.as_ref().to_owned()).collect();
        let userinfo = access_userinfo(url).unwrap_or("");
        let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
        let decode = |s: &str| percent_decode(s).unwrap_or_else(|| s.to_owned());
        values.extend([decode(user), decode(password)]);
        require_long(&values)?;
        for raw in [userinfo, user, password] {
            values.push(raw.to_owned());
        }
        values.push(decode(userinfo));
        values.push(format!("{}:{}", decode(user), decode(password)));
        Ok(Self::build(values))
    }
    fn build(mut list: Vec<String>) -> Self {
        list.sort();
        list.dedup();
        let mut patterns: Vec<Pattern> = list.iter().map(|v| v.chars().map(|c| vec![c]).collect()).collect();
        for v in &list {
            patterns.extend(base64_patterns(v.as_bytes()));
        }
        let mut set = Self { values: list.len(), patterns, ..Self::default() };
        set.index_starts();
        set
    }
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
    /// `text` with every spelling of every value replaced by [`MASK`], rescanned until none is left (a
    /// mask can close a gap that completes another spelling). Every match spans at least
    /// [`MIN_CREDENTIAL_CHARS`] characters other than whitespace (a base64 form at least
    /// [`MIN_CREDENTIAL_CHARS`] digits) and becomes three, so each rescan shortens the text and the loop
    /// ends.
    pub fn mask(&self, text: &str) -> String {
        let mut current = text.to_owned();
        if self.patterns.is_empty() {
            return current;
        }
        loop {
            let next = self.mask_once(&current);
            if next == current {
                return next;
            }
            current = next;
        }
    }
    /// One scan: every match of every pattern from every position, and each run of characters any match
    /// covers becomes one [`MASK`]. Not leftmost-longest: a longest match may end inside the next
    /// spelling (`…\ \u0070…` reads `\ \` as an escaped backslash), and replacing it would break that
    /// spelling apart; the union masks both.
    /// A match may start or end at the middle of a reference that spells two characters (FIX8): it then
    /// covers the whole reference.
    pub(crate) fn sensitive_ranges(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        if self.is_empty() || text.len() < MIN_CREDENTIAL_CHARS { return vec![]; }
        let lattice = Lattice::new(text);
        let n = lattice.s.len();
        // the furthest end of any match starting at each text position
        let mut reach: Vec<Option<usize>> = vec![None; n];
        for q in (0..n).chain(n + 1..lattice.nodes()) {
            let candidates = lattice.at(DEPTH, q).iter()
                .flat_map(|(c, _)| self.starts.get(c).into_iter().flatten())
                .chain(self.blank_starts.iter());
            if let Some(end) = candidates.filter_map(|&i| lattice.match_at(&self.patterns[i], q)).max() {
                let start = lattice.start_of(q);
                reach[start] = reach[start].max(Some(end));
            }
        }
        let mut ranges: Vec<std::ops::Range<usize>> = vec![];
        let offsets: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
        for (q, reach) in reach.into_iter().enumerate() {
            if let Some(end) = reach {
                let range = offsets[q]..offsets[end];
                if let Some(last) = ranges.last_mut() && range.start < last.end {
                    last.end = last.end.max(range.end);
                } else { ranges.push(range); }
            }
        }
        ranges
    }
    fn mask_once(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut from = 0;
        for range in self.sensitive_ranges(text) {
            out.push_str(&text[from..range.start]);
            out.push_str(MASK);
            from = range.end;
        }
        out.push_str(&text[from..]);
        out
    }
}

/// The userinfo of `scheme://userinfo@authority…`: up to the authority's last `@`.
fn access_userinfo(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let at = rest[..end].rfind('@')?;
    Some(&rest[..at])
}
/// Refuses any credential component with fewer than [`MIN_CREDENTIAL_CHARS`] characters other than
/// whitespace.
fn require_long(components: &[String]) -> FbResult<()> {
    if components.iter().any(|v| v.chars().filter(|&c| !is_blank(c)).count() < MIN_CREDENTIAL_CHARS) {
        return Err(err(TOO_SHORT));
    }
    Ok(())
}
/// `%XX` decoding to UTF-8, or None when nothing was encoded or the bytes are not UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    if !s.contains('%') {
        return None;
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Whitespace and invisible characters: a gap between characters, and skipped between escape tokens.
pub fn is_blank(c: char) -> bool {
    text::is_space(c) || c.is_whitespace() || matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{AD}')
}

/// What a numeric HTML reference past U+10FFFF is read as: the digit run keeps going, the value stops.
const PAST_UNICODE: u32 = 0x11_0000;
/// Stands for "nothing" (a code point `html.unescape` drops): blank, so a gap between characters.
const NOTHING: char = '\u{200B}';

/// The characters a numeric HTML reference with value `v` reads as, under `html.unescape` and WHATWG:
/// the Windows-1252 remap for `0x80..=0x9F` (and NUL, CR), U+FFFD for surrogates and values past
/// U+10FFFF, and for the code points `html.unescape` drops both nothing and the code point itself
/// (WHATWG keeps it).
fn html_numeric(v: u32) -> Vec<char> {
    if let Ok(i) = html_tables::REMAP.binary_search_by_key(&v, |&(k, _)| k) {
        return vec![html_tables::REMAP[i].1];
    }
    let Some(c) = char::from_u32(v).filter(|_| v < PAST_UNICODE) else {
        return vec!['\u{FFFD}'];
    };
    if html_tables::DROPPED.iter().any(|&(lo, hi)| (lo..=hi).contains(&v)) { vec![NOTHING, c] } else { vec![c] }
}

/// The node between the two characters of a reference that spells two (FIX8): the reference spans
/// `start..end` of the text, and the node's one edge is `(second character, end)`.
struct Middle {
    start: usize,
    edge: (char, usize),
}

/// Every way each position of a text can spell one character, by escape depth. Nodes `0..=n` are
/// positions of the text; node `n + 1 + i` is `middles[i]`, inside one reference, the same at every depth.
struct Lattice {
    s: Vec<char>,
    /// `edges[d][q]`: (character, end) spelled from `q` with escapes nested at most `d` deep.
    edges: Vec<Vec<Vec<(char, usize)>>>,
    middles: Vec<Middle>,
    /// `gaps[q]`: ends of one gap token (whitespace in any spelling, an HTML tag) starting at node `q`.
    gaps: Vec<Vec<usize>>,
}
impl Lattice {
    fn new(text: &str) -> Self {
        let s: Vec<char> = text.chars().collect();
        let n = s.len();
        let mut l = Lattice { edges: vec![(0..n).map(|q| vec![(s[q], q + 1)]).collect()], s, middles: vec![], gaps: vec![] };
        for d in 1..=DEPTH {
            let mut level = Vec::with_capacity(n);
            for q in 0..n {
                let (mut edges, pairs) = l.decode_at(d, q);
                for (first, second, end) in pairs {
                    let node = l.middle(q, second, end);
                    edges.push((first, node));
                }
                edges.sort_unstable();
                edges.dedup();
                level.push(edges);
            }
            l.edges.push(level);
        }
        l.gaps = (0..l.nodes())
            .map(|q| {
                let mut ends: Vec<usize> = l.at(DEPTH, q).iter().filter(|(c, _)| is_blank(*c)).map(|&(_, e)| e).collect();
                if q < n
                    && l.s[q] == '<'
                    && let Some(k) = l.s[q + 1..].iter().position(|&c| c == '>' || c == '<')
                    && l.s[q + 1 + k] == '>'
                {
                    ends.push(q + k + 2);
                }
                ends
            })
            .collect();
        l
    }
    /// Text positions and middle nodes.
    fn nodes(&self) -> usize {
        self.s.len() + 1 + self.middles.len()
    }
    /// The middle node of a reference from `start` to `end` whose second character is `second`.
    fn middle(&mut self, start: usize, second: char, end: usize) -> usize {
        let base = self.s.len() + 1;
        if let Some(i) = self.middles.iter().position(|m| m.start == start && m.edge == (second, end)) {
            return base + i;
        }
        self.middles.push(Middle { start, edge: (second, end) });
        base + self.middles.len() - 1
    }
    /// The text position a node stands for at the end of a match: a middle node stands for the end of its
    /// reference, so a match ending inside a reference masks all of it.
    fn end_of(&self, node: usize) -> usize {
        node.checked_sub(self.s.len() + 1).map_or(node, |i| self.middles[i].edge.1)
    }
    /// The text position a node stands for at the start of a match: a middle node stands for the start of
    /// its reference.
    fn start_of(&self, node: usize) -> usize {
        node.checked_sub(self.s.len() + 1).map_or(node, |i| self.middles[i].start)
    }
    fn at(&self, d: usize, q: usize) -> &[(char, usize)] {
        match q.checked_sub(self.s.len() + 1) {
            Some(i) => self.middles.get(i).map_or(&[], |m| std::slice::from_ref(&m.edge)),
            None => self.edges[d].get(q).map_or(&[], |v| v.as_slice()),
        }
    }
    fn skip_blank(&self, mut q: usize) -> usize {
        while q < self.s.len() && is_blank(self.s[q]) {
            q += 1;
        }
        q
    }
    /// Ends of one `want` token at depth `d` from each of `starts` (whitespace allowed before it).
    fn token(&self, d: usize, starts: &[usize], want: impl Fn(char) -> bool) -> Vec<(char, usize)> {
        let mut out = vec![];
        for &q in starts {
            let q = self.skip_blank(q);
            out.extend(self.at(d, q).iter().filter(|(c, _)| want(*c)).copied());
        }
        out
    }
    fn ends(&self, d: usize, starts: &[usize], want: char) -> Vec<usize> {
        let mut v: Vec<usize> = self.token(d, starts, |c| c == want).into_iter().map(|(_, e)| e).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
    /// Runs of `min..=max` digits in `radix` (each digit a token at depth `d`) from `start`: (value, end).
    /// Fixed-width escapes pass `min == max`. HTML numeric references take the longest run of any length
    /// instead ([`Lattice::html_digits`]), so `&#107;` is never read as `&#10` and `7;`.
    fn digits(&self, d: usize, start: usize, radix: u32, min: usize, max: usize) -> Vec<(u32, usize)> {
        let mut out = vec![];
        let mut states = vec![(0u32, start)];
        for count in 1..=max {
            let mut next = vec![];
            for &(v, q) in &states {
                for (c, e) in self.token(d, &[q], |c| c.is_digit(radix)) {
                    if let Some(v) = v.checked_mul(radix).and_then(|v| v.checked_add(c.to_digit(radix).unwrap_or(0))) {
                        next.push((v, e));
                    }
                }
            }
            next.sort_unstable();
            next.dedup();
            if count >= min {
                out.extend(next.iter().copied());
            }
            if next.is_empty() {
                break;
            }
            states = next;
        }
        out
    }
    /// The digits of a numeric HTML reference from `start`: the longest runs (those no further digit
    /// token extends), of any length, so leading zeros never end the run. The value saturates at
    /// [`PAST_UNICODE`]; the run is bounded by the text, the states by the distinct (value, end) pairs.
    fn html_digits(&self, d: usize, start: usize, radix: u32) -> Vec<(u32, usize)> {
        let mut out = vec![];
        let mut states = vec![(0u32, start)];
        while !states.is_empty() {
            let mut next = vec![];
            for &(v, q) in &states {
                // a run ends where no digit follows at once. Whitespace between digit tokens is read
                // through as a gap, which only adds runs. A digit spelled by another HTML reference
                // never extends a run (`&#81&#49;` is `Q1`: one reference cannot spell the digits of
                // the one before it), and it starts where an `&` is spelled one level down.
                let html_next = d > 0 && self.at(d - 1, q).iter().any(|(c, _)| *c == '&');
                if q != start && (html_next || !self.at(d, q).iter().any(|(c, _)| c.is_digit(radix))) {
                    out.push((v, q));
                }
                let more = self.token(d, &[q], |c| c.is_digit(radix));
                next.extend(more.into_iter().map(|(c, e)| ((v * radix + c.to_digit(radix).unwrap_or(0)).min(PAST_UNICODE), e)));
            }
            next.sort_unstable();
            next.dedup();
            states = next;
        }
        out
    }
    /// The named references spelled from `start` (letters and digits as tokens at depth `d`): (what it
    /// spells, end), the `;` left to the caller.
    fn html_named(&self, d: usize, start: usize) -> Vec<(&'static str, usize)> {
        let names = html_tables::NAMED;
        let mut out = vec![];
        let mut states = vec![(String::new(), start)];
        while !states.is_empty() {
            let mut next = vec![];
            for (prefix, q) in &states {
                for (c, e) in self.token(d, &[*q], |c| c.is_ascii_alphanumeric()) {
                    let name = format!("{prefix}{c}");
                    let i = names.partition_point(|&(k, _)| k < name.as_str());
                    match names.get(i) {
                        Some(&(k, spelled)) if k == name => {
                            out.push((spelled, e));
                            next.push((name, e));
                        }
                        Some(&(k, _)) if k.starts_with(&name) => next.push((name, e)),
                        _ => {}
                    }
                }
            }
            next.sort_unstable();
            next.dedup();
            states = next;
        }
        out
    }
    /// One percent-encoded byte from `start`: `%` (any spelling at depth `d`) and two hex digits.
    fn pct_byte(&self, d: usize, start: usize) -> Vec<(u8, usize)> {
        let mut out = vec![];
        for (_, e) in self.at(d, start).iter().filter(|(c, _)| *c == '%') {
            out.extend(self.digits(d, *e, 16, 2, 2).into_iter().map(|(v, e)| (v as u8, e)));
        }
        out
    }
    /// Every character spelled at `q` with escapes nested at most `d` deep; tokens inside an escape are
    /// read at depth `d - 1`. Also the references that spell two characters: (first, second, end), for
    /// the caller to give a middle node.
    #[allow(clippy::type_complexity)]
    fn decode_at(&self, d: usize, q: usize) -> (Vec<(char, usize)>, Vec<(char, char, usize)>) {
        let sub = d - 1;
        let mut out = self.edges[sub][q].clone();
        let mut pairs = vec![];
        if self.s[q] == '+' {
            out.push((' ', q + 1));
        }
        // percent-encoded UTF-8
        for (lead, end) in self.pct_byte(sub, q) {
            let need = match lead {
                0x00..=0x7F => 0,
                0xC2..=0xDF => 1,
                0xE0..=0xEF => 2,
                0xF0..=0xF4 => 3,
                _ => continue,
            };
            let mut states = vec![(vec![lead], end)];
            for _ in 0..need {
                let mut next = vec![];
                for (bytes, at) in &states {
                    for (b, e) in self.pct_byte(sub, self.skip_blank(*at)) {
                        if b & 0xC0 == 0x80 {
                            let mut v = bytes.clone();
                            v.push(b);
                            next.push((v, e));
                        }
                    }
                }
                states = next;
            }
            for (bytes, e) in states {
                if let Ok(s) = std::str::from_utf8(&bytes)
                    && let Some(c) = s.chars().next()
                {
                    out.push((c, e));
                }
            }
        }
        // backslash escapes (JSON, Rust, Python)
        for &(_, e1) in self.at(sub, q).iter().filter(|(c, _)| *c == '\\') {
            for (c, e2) in self.token(sub, &[e1], |_| true) {
                let short = match c {
                    '/' | '"' | '\\' | '\'' => Some(c),
                    'n' => Some('\n'),
                    'r' => Some('\r'),
                    't' => Some('\t'),
                    'b' => Some('\u{8}'),
                    'f' => Some('\u{C}'),
                    _ => None,
                };
                if let Some(ch) = short {
                    out.push((ch, e2));
                }
                match c {
                    'u' => {
                        for (unit, e3) in self.digits(sub, e2, 16, 4, 4) {
                            if (0xD800..0xDC00).contains(&unit) {
                                for e4 in self.ends(sub, &[e3], '\\') {
                                    for e5 in self.ends(sub, &[e4], 'u') {
                                        for (low, e6) in self.digits(sub, e5, 16, 4, 4) {
                                            if (0xDC00..0xE000).contains(&low) {
                                                out.extend(char::from_u32(0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)).map(|ch| (ch, e6)));
                                            }
                                        }
                                    }
                                }
                            } else {
                                out.extend(char::from_u32(unit).map(|ch| (ch, e3)));
                            }
                        }
                        for e3 in self.ends(sub, &[e2], '{') {
                            for (v, e4) in self.digits(sub, e3, 16, 1, 8) {
                                for e5 in self.ends(sub, &[e4], '}') {
                                    out.extend(char::from_u32(v).map(|ch| (ch, e5)));
                                }
                            }
                        }
                    }
                    'x' => out.extend(self.digits(sub, e2, 16, 2, 2).into_iter().filter_map(|(v, e)| char::from_u32(v).map(|ch| (ch, e)))),
                    'U' => out.extend(self.digits(sub, e2, 16, 8, 8).into_iter().filter_map(|(v, e)| char::from_u32(v).map(|ch| (ch, e)))),
                    _ => {}
                }
            }
        }
        // HTML character references
        for &(_, e1) in self.at(sub, q).iter().filter(|(c, _)| *c == '&') {
            let mut numeric = vec![];
            for e2 in self.ends(sub, &[e1], '#') {
                numeric.extend(self.html_digits(sub, e2, 10));
                for (_, e3) in self.token(sub, &[e2], |c| c == 'x' || c == 'X') {
                    numeric.extend(self.html_digits(sub, e3, 16));
                }
            }
            let mut found: Vec<(char, usize)> = numeric.into_iter().flat_map(|(v, e)| html_numeric(v).into_iter().map(move |ch| (ch, e))).collect();
            for (spelled, e) in self.html_named(sub, e1) {
                let mut chars = spelled.chars();
                match (chars.next(), chars.next()) {
                    (Some(ch), None) => found.push((ch, e)),
                    // two characters: read only with the `;`
                    (Some(first), Some(second)) => pairs.extend(self.ends(sub, &[e], ';').into_iter().map(|e| (first, second, e))),
                    _ => {}
                }
            }
            for (ch, e) in found {
                out.push((ch, e));
                out.extend(self.ends(sub, &[e], ';').into_iter().map(|e| (ch, e)));
            }
        }
        out.sort_unstable();
        out.dedup();
        pairs.sort_unstable();
        pairs.dedup();
        (out, pairs)
    }
    /// Every position reachable from `starts` through gaps.
    fn closure(&self, mut starts: Vec<usize>) -> Vec<usize> {
        let mut i = 0;
        while i < starts.len() {
            if let Some(ends) = self.gaps.get(starts[i]) {
                for &e in ends {
                    if !starts.contains(&e) {
                        starts.push(e);
                    }
                }
            }
            i += 1;
        }
        starts
    }
    /// The longest end of `p` spelled from node `q`, if any, as a text position ([`Lattice::end_of`]).
    fn match_at(&self, p: &Pattern, q: usize) -> Option<usize> {
        let mut set = vec![q];
        for (k, allowed) in p.iter().enumerate() {
            let mut next: Vec<usize> = vec![];
            let blank = allowed.iter().all(|&c| is_blank(c));
            if blank {
                // the secret's own whitespace: one whitespace spelling of any kind, or none (gaps between
                // characters add more); never a run of gaps at the value's end, which could end the match
                // inside the next spelling
                next = set.clone();
            }
            for &at in &set {
                next.extend(self.at(DEPTH, at).iter().filter(|(c, _)| allowed.contains(c) || blank && is_blank(*c)).map(|&(_, e)| e));
            }
            next.sort_unstable();
            next.dedup();
            if next.is_empty() {
                return None;
            }
            set = if k + 1 < p.len() { self.closure(next) } else { next };
        }
        set.into_iter().map(|e| self.end_of(e)).max().filter(|&end| end > self.start_of(q))
    }
}

const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
/// The base64 digits fully determined by `bytes` when they start `offset` bytes into a 3-byte group, as
/// 6-bit values. The first and last digits, mixed with neighbouring bytes, are left out.
fn base64_core(bytes: &[u8], offset: usize) -> Vec<u8> {
    let mut data = vec![0u8; offset];
    data.extend_from_slice(bytes);
    let start = (offset * 8).div_ceil(6);
    let end = data.len() * 8 / 6;
    (start..end)
        .map(|i| (0..6).fold(0u8, |v, k| (v << 1) | ((data[(i * 6 + k) / 8] >> (7 - (i * 6 + k) % 8)) & 1)))
        .collect()
}
/// base64 and base64url of `bytes` at each alignment (the two alphabets differ only in digits 62, 63).
fn base64_patterns(bytes: &[u8]) -> Vec<Pattern> {
    (0..3)
        .map(|offset| base64_core(bytes, offset))
        .filter(|core| core.len() >= MIN_CREDENTIAL_CHARS)
        .map(|core| {
            core.iter()
                .map(|&v| {
                    let (s, u) = (STD[v as usize] as char, URL[v as usize] as char);
                    if s == u { vec![s] } else { vec![s, u] }
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(bytes: &[u8], offset: usize) -> String {
        base64_core(bytes, offset).iter().map(|&v| STD[v as usize] as char).collect()
    }

    #[test]
    fn base64_cores_sit_inside_every_alignment() {
        assert_eq!(core(b"user:pass", 0), "dXNlcjpwYXNz");
        assert!("eHVzZXI6cGFzcw".contains(&core(b"user:pass", 1)));
        assert!("eHh1c2VyOnBhc3M".contains(&core(b"user:pass", 2)));
    }

    #[test]
    fn debug_never_prints_a_value() {
        let s = SecretSet::masking_from_access_url("https://someuser-name:somepassword@bridge.test/simplefin", ["token-abcdef"]).unwrap();
        let shown = format!("{s:?}");
        assert!(!shown.contains("someuser") && !shown.contains("somepassword") && !shown.contains("token"), "{shown}");
    }

    /// R7: numeric references read the way `html.unescape` reads them (vectors from Python 3.13).
    #[test]
    fn html_references_as_python_reads_them() {
        let s = SecretSet::masking_new(["r7ownsecret-long"]).unwrap();
        for zeros in [0, 1, 9, 10, 11, 23, 40, 200] {
            let z = "0".repeat(zeros);
            for spelled in [format!("&#{z}114;7ownsecret-long"), format!("&#x{z}72;7ownsecret-long"), format!("&#X{z}72;7ownsecret-long")] {
                assert_eq!(s.mask(&format!("a {spelled} b")), "a *** b", "{spelled}");
            }
        }
        // without `;`, where html.unescape still ends the run: `&#114` before a letter
        assert_eq!(SecretSet::masking_new(["r-own-secret-long"]).unwrap().mask("a &#0000114-own-secret-long b"), "a *** b");
        // Windows-1252 remap: html.unescape("&#128;") == "€", "&#x9F;" == "Ÿ", "&#150;" == "–"
        let euro = SecretSet::masking_new(["r7€pass–wordŸ"]).unwrap();
        assert_eq!(euro.mask("a r7&#128;pass&#x96;word&#0000159; b"), "a *** b");
        assert_eq!(euro.mask("a r7&#x80pass&#150word&#x9f b"), "a *** b");
        // the raw C1 code point is not what HTML reads
        assert_eq!(euro.mask("r7\u{80}pass"), "r7\u{80}pass");
        // named references, `;` optional where html.unescape allows it
        let named = SecretSet::masking_new(["p!ss€w<rd-long"]).unwrap();
        assert_eq!(named.mask("a p&excl;ss&euro;w&lt;rd-long b"), "a *** b");
        assert_eq!(named.mask("a p&excl;ss&euro;w&ltrd-long b"), "a *** b");
        // values past U+10FFFF and surrogates read as U+FFFD, NUL too; dropped code points read as nothing
        let odd = SecretSet::masking_new(["ab\u{FFFD}cdefghijkl"]).unwrap();
        for r in ["&#99999999999999999999;", "&#xD800;", "&#0;", "&#x110000;"] {
            assert_eq!(odd.mask(&format!("ab{r}cdefghijkl")), "***", "{r}");
        }
        assert_eq!(SecretSet::masking_new(["abcdefghijklmn"]).unwrap().mask("abcdef&#1;ghij&#xFFFE;klmn"), "***");
    }

    /// R8: the 93 named references that spell two characters, read only with their `;`; a match may
    /// start or end between the two characters, and then masks the whole reference.
    #[test]
    fn two_character_references() {
        let s = SecretSet::masking_new(["R8prefixfjsecretlong"]).unwrap();
        assert_eq!(s.mask("a R8prefix&fjlig;secretlong b"), "a *** b");
        assert_eq!(s.mask("a R8prefix&amp;fjlig;secretlong b"), "a *** b");
        assert_eq!(s.mask("a R8prefix%26fjlig%3Bsecretlong b"), "a *** b");
        assert_eq!(s.mask("a R8prefix&fjligsecretlong b"), "a R8prefix&fjligsecretlong b", "no `;`, not a reference");
        // the value ends after the `f`, or starts at the `j`, of `&fjlig;`
        assert_eq!(SecretSet::masking_new(["R8prefix-endsinf"]).unwrap().mask("a R8prefix-endsin&fjlig;ust b"), "a ***ust b");
        assert_eq!(SecretSet::masking_new(["jstartsthevalue"]).unwrap().mask("a xyz&fjlig;startsthevalue b"), "a xyz*** b");
        // a second character that is a combining mark, and one reference after another
        let nvlt = SecretSet::masking_new(["R8<\u{20d2}>\u{20d2}=\u{20e5}secretlong"]).unwrap();
        assert_eq!(nvlt.mask("x R8&nvlt;&nvgt;&bne;secretlong y"), "x *** y");
        // two blank characters: a gap between characters of the value
        assert_eq!(SecretSet::masking_new(["abcdefghijklmn"]).unwrap().mask("abcdefg&ThickSpace;hijklmn"), "***");
    }

    #[test]
    fn short_components_are_refused() {
        let refused = |r: FbResult<SecretSet>| r.unwrap_err().to_string();
        assert!(refused(SecretSet::masking_from_credentials("ab", "cdefghijklmnop", Vec::<String>::new())).contains(TOO_SHORT));
        assert!(refused(SecretSet::masking_from_credentials("abcdefghijklmn", "cdefghijklmnop", ["short-token"])).contains(TOO_SHORT));
        assert!(refused(SecretSet::masking_from_access_url("https://r7username-long:q7!@r7.test/access", Vec::<String>::new())).contains(TOO_SHORT));
        // counted after percent-decoding, whitespace left out
        assert!(refused(SecretSet::masking_from_access_url("https://%41%42%43%44%45%46:abcdefghijklmn@r7.test/access", Vec::<String>::new())).contains(TOO_SHORT));
        assert!(refused(SecretSet::masking_new(["abc def ghi jk"])).contains(TOO_SHORT));
        assert!(refused(SecretSet::masking_from_access_url("https://bridge.test/simplefin", Vec::<String>::new())).contains(TOO_SHORT));
        assert!(SecretSet::masking_new(["abcdefghijkl"]).is_ok());
        let s = SecretSet::masking_from_credentials("abcdefghijkl", "mnopqrstuvwx", Vec::<String>::new()).unwrap();
        assert_eq!(s.mask("abcdefghijkl and mnopqrstuvwx and abcdefghijkl:mnopqrstuvwx"), "*** and *** and ***");
    }

    #[test]
    fn spellings_of_one_secret() {
        let s = SecretSet::masking_new(["pa/ss w0rd/long"]).unwrap();
        for spelled in [
            "pa/ss w0rd",
            "pa%2Fss%20w0rd",
            "pa%2fss+w0rd",
            "pa%252Fss%2520w0rd",
            r"pa\/ss w0rd",
            r"pa/ss w0rd",
            r"pa\\u002fss w0rd",
            "&#112;&#x61;&#47;ss&#32;w0rd",
            "pa&amp;#47;ss w0rd",
            "p a / s s\u{2003}w\u{200B}0 r d",
            "pa<b>/ss</b> w0rd",
            "pa%2F ss w0\nrd",
        ] {
            let masked = s.mask(&format!("before {spelled}/long after"));
            assert_eq!(masked, "before *** after", "{spelled}");
        }
        // base64 keeps only the digits mixed with what follows (and the padding)
        for (spelled, rest) in [("cGEvc3MgdzByZC9sb25n", ""), ("eHBhL3NzIHcwcmQvbG9uZw==", "w=="), ("eHBhL3NzIHcwcmQvbG9uZw", "w")] {
            let masked = s.mask(&format!("before {spelled} after"));
            assert!(!masked.contains(spelled) && masked.contains("***") && masked.ends_with(&format!("{rest} after")), "{spelled} -> {masked}");
        }
        assert_eq!(s.mask("PA/SS W0RD/LONG"), "PA/SS W0RD/LONG", "letters are case-sensitive");
    }

    #[test]
    fn basic_header_and_access_url() {
        let s = SecretSet::masking_from_access_url("https://us%C3%A9r-name-xy:p%40ss%20word-xyz@bridge.test/simplefin", Vec::<String>::new()).unwrap();
        let basic = "Authorization: Basic dXPDqXItbmFtZS14eTpwQHNzIHdvcmQteHl6";
        let masked = s.mask(basic);
        assert!(!masked.contains("dXPDqXItbmFtZS14eTpwQHNzIHdvcmQteHl"), "{masked}");
        assert_eq!(s.mask("https://us%C3%A9r-name-xy:p%40ss%20word-xyz@bridge.test/x"), "https://***@bridge.test/x");
        assert_eq!(s.mask("user usér-name-xy pass p@ss word-xyz"), "user *** pass ***");
    }
}
