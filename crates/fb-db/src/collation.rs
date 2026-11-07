//! Embedded, OS-independent ordering and simple Unicode casing. See COLLATION.md.
use crate::DbError;
use icu_casemap::CaseMapper;
use icu_collator::{
    Collator, CollatorBorrowed, CollatorPreferences,
    options::{AlternateHandling, CaseLevel, CollatorOptions, MaxVariable, Strength},
    preferences::CollationCaseFirst,
};
use icu_locale::locale;
use rusqlite::Connection;
use std::{cmp::Ordering, sync::OnceLock};

fn collator(tertiary: bool) -> CollatorBorrowed<'static> {
    let mut options = CollatorOptions::default();
    options.strength = Some(if tertiary {
        Strength::Tertiary
    } else {
        Strength::Secondary
    });
    options.alternate_handling = Some(AlternateHandling::Shifted);
    options.max_variable = Some(MaxVariable::Symbol);
    options.case_level = Some(CaseLevel::On);
    let mut prefs: CollatorPreferences = locale!("en-US").into();
    prefs.case_first = Some(CollationCaseFirst::False);
    Collator::try_new(prefs, options).expect("compiled en-US collation data")
}

/// Total product order, with PostgreSQL's deterministic UTF-8 byte tie-break.
pub fn compare(a: &str, b: &str) -> Ordering {
    static SORT: OnceLock<CollatorBorrowed<'static>> = OnceLock::new();
    SORT.get_or_init(|| collator(false))
        .compare(a, b)
        .then_with(|| a.as_bytes().cmp(b.as_bytes()))
}

/// The sole accepted oracle-order exception: equal at tertiary strength with
/// alternate=Shifted. No byte tie-break here: punctuation is intentionally ignored.
pub fn punctuation_tie(a: &str, b: &str) -> bool {
    static TIES: OnceLock<CollatorBorrowed<'static>> = OnceLock::new();
    TIES.get_or_init(|| collator(true)).compare(a, b) == Ordering::Equal
}

/// Unicode simple casing (one scalar to one scalar), never locale or full expansions.
pub fn lower(text: &str) -> String {
    let mapper = CaseMapper::new();
    text.chars().map(|c| mapper.simple_lowercase(c)).collect()
}
pub fn upper(text: &str) -> String {
    let mapper = CaseMapper::new();
    text.chars().map(|c| mapper.simple_uppercase(c)).collect()
}

pub fn register(conn: &Connection) -> Result<(), DbError> {
    conn.create_collation("fb_text", compare)?;
    for (name, mapping) in [
        ("fb_lower", lower as fn(&str) -> String),
        ("fb_upper", upper),
    ] {
        conn.create_scalar_function(
            name,
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            move |ctx| {
                let text: String = ctx.get(0)?;
                Ok(mapping(&text))
            },
        )?;
    }
    Ok(())
}
