//! Which build this is. The native module (fb-node) stamps the commit, tree state, date and
//! compiled-in features at build time and registers them here once at load; tools and tests that
//! never register see "unknown" values, never a made-up commit.
use serde_json::{Value, json};
use std::sync::{OnceLock, RwLock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    /// The Rust crates' version (`0.1.0`); the shell's own version is registered separately.
    pub crate_version: &'static str,
    /// Full commit id, or "unknown" when the source had no git metadata.
    pub commit: String,
    /// Whether the working tree had changes at build time; None when unknown.
    pub dirty: Option<bool>,
    /// Build date, `YYYY-MM-DD` (UTC; `SOURCE_DATE_EPOCH` when set).
    pub date: String,
    /// "release" or "debug".
    pub profile: &'static str,
    /// Optional capabilities compiled in, in a fixed order.
    pub features: Vec<&'static str>,
    /// Schema migrations this build knows (an open book is always at this version).
    pub migrations: usize,
    pub rustc: String,
}

impl Build {
    fn unknown() -> Build {
        Build {
            crate_version: env!("CARGO_PKG_VERSION"),
            commit: "unknown".into(),
            dirty: None,
            date: "unknown".into(),
            profile: if cfg!(debug_assertions) { "debug" } else { "release" },
            features: vec![],
            migrations: known_migrations(),
            rustc: "unknown".into(),
        }
    }

    /// The first 10 characters of the commit, or "unknown".
    pub fn short_commit(&self) -> &str {
        if self.commit.len() >= 10 && self.commit.chars().all(|c| c.is_ascii_hexdigit()) { &self.commit[..10] } else { "unknown" }
    }

    pub fn json(&self) -> Value {
        json!({
            "crate_version": self.crate_version, "commit": self.commit, "dirty": self.dirty, "date": self.date,
            "profile": self.profile, "features": self.features, "migrations": self.migrations, "rustc": self.rustc,
            "shell_version": shell_version(), "version": short_version(),
        })
    }
}

/// Schema migrations this build knows. Use this, not `build().migrations`, while assembling the
/// identity to register: `build()` would fix the identity as unknown first.
pub fn known_migrations() -> usize {
    fb_db::MIGRATIONS.len()
}

static BUILD: OnceLock<Build> = OnceLock::new();
static SHELL: RwLock<String> = RwLock::new(String::new());
static OS: RwLock<String> = RwLock::new(String::new());

/// The operating system's name and version as the shell reports it (`Debian GNU/Linux 13`).
pub fn set_os(os: &str) {
    *OS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = crate::scrub::os_name(os);
}

pub fn os() -> String {
    let os = OS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    if os.is_empty() { std::env::consts::OS.to_owned() } else { os }
}

/// Register the stamped identity (once; later calls are ignored).
pub fn set_build(build: Build) {
    let _ = BUILD.set(build);
}

pub fn build() -> &'static Build {
    BUILD.get_or_init(Build::unknown)
}

/// The desktop shell's own version (Electron's package metadata), shown first.
pub fn set_shell_version(version: &str) {
    let clean: String = version.chars().filter(|c| c.is_ascii_graphic()).take(40).collect();
    *SHELL.write().unwrap_or_else(std::sync::PoisonError::into_inner) = clean;
}

fn shell_version() -> String {
    let v = SHELL.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    if v.is_empty() { build().crate_version.to_owned() } else { v }
}

/// The one-line version the app shows: `0.1.0-d4 (49417ebd52)`, `0.1.0-d4 (49417ebd52, modified)`
/// or `0.1.0-d4 (source build)` when the commit is unknown.
pub fn short_version() -> String {
    let b = build();
    match (b.short_commit(), b.dirty) {
        ("unknown", _) => format!("{} (source build)", shell_version()),
        (commit, Some(true)) => format!("{} ({commit}, modified)", shell_version()),
        (commit, _) => format!("{} ({commit})", shell_version()),
    }
}

/// What a backup records as the version that made it (its manifest's `app_version` text).
pub fn record_label() -> String {
    let b = build();
    let tree = match b.dirty {
        Some(true) => " modified",
        Some(false) => "",
        None => " (tree state unknown)",
    };
    let features = if b.features.is_empty() { "none".to_owned() } else { b.features.join(",") };
    format!("FreedomBooks {}; commit {}{tree}; built {} ({}); schema {}; features {features}", shell_version(), b.commit, b.date, b.profile, b.migrations)
}

/// The build lines of "Report a problem" and the diagnostic summary.
pub fn details() -> Vec<String> {
    let b = build();
    let tree = match b.dirty {
        Some(true) => ", with uncommitted changes",
        Some(false) => "",
        None => "",
    };
    vec![
        format!("Build: commit {}{tree}, built {} ({}, {})", b.commit, b.date, b.profile, b.rustc),
        format!("Features: {}", if b.features.is_empty() { "none".to_owned() } else { b.features.join(", ") }),
        format!("Book format: {} schema migrations", b.migrations),
    ]
}
