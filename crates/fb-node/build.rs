//! Build identity (CRASHLOG). Each value comes from, in order:
//! - `FB_BUILD_COMMIT` / `FB_BUILD_DIRTY` (`1`/`0`) / `FB_BUILD_DATE` (`YYYY-MM-DD`) when set:
//!   `electron/scripts/build-native.cjs` sets all three from the tree it builds, so a desktop
//!   build's identity is exact, and a packager without git metadata can supply them;
//! - otherwise git (`rev-parse HEAD`, `status --porcelain`), rerun when HEAD, the branch ref or the
//!   index changes. Edits to tracked files that git has not seen do not rerun this script, so a
//!   plain `cargo build` can call a since-edited tree clean; the desktop build path never relies on it;
//! - otherwise "unknown": a source tree without git metadata still builds.
//!
//! The date is `SOURCE_DATE_EPOCH` (reproducible builds) or today (UTC).
use std::process::Command;

fn git(root: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn civil(days: i64) -> (i64, u32, u32) {
    // Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn main() {
    napi_build::setup();
    for var in ["FB_BUILD_COMMIT", "FB_BUILD_DIRTY", "FB_BUILD_DATE", "SOURCE_DATE_EPOCH"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default()).join("../..");
    let in_git = git(&root, &["rev-parse", "--is-inside-work-tree"]).is_some_and(|s| s == "true");
    if in_git {
        for path in ["HEAD", "index", "packed-refs"] {
            if let Some(p) = git(&root, &["rev-parse", "--git-path", path]) {
                let p = std::path::Path::new(&p);
                let p = if p.is_absolute() { p.to_owned() } else { root.join(p) };
                if p.exists() {
                    println!("cargo:rerun-if-changed={}", p.display());
                }
            }
        }
        if let Some(branch) = git(&root, &["symbolic-ref", "-q", "HEAD"])
            && let Some(p) = git(&root, &["rev-parse", "--git-path", &branch])
        {
            let p = std::path::Path::new(&p);
            let p = if p.is_absolute() { p.to_owned() } else { root.join(p) };
            if p.exists() {
                println!("cargo:rerun-if-changed={}", p.display());
            }
        }
    }
    let valid_commit = |c: &str| c.len() == 40 && c.chars().all(|c| c.is_ascii_hexdigit());
    let commit = std::env::var("FB_BUILD_COMMIT").ok().filter(|c| valid_commit(c))
        .or_else(|| if in_git { git(&root, &["rev-parse", "HEAD"]).filter(|c| valid_commit(c)) } else { None })
        .unwrap_or_else(|| "unknown".into());
    let dirty = match std::env::var("FB_BUILD_DIRTY").ok().as_deref() {
        Some("1") => "1".to_owned(),
        Some("0") => "0".to_owned(),
        _ if in_git && commit != "unknown" => match git(&root, &["status", "--porcelain"]) {
            Some(s) if s.is_empty() => "0".into(),
            Some(_) => "1".into(),
            None => "unknown".into(),
        },
        _ => "unknown".into(),
    };
    let date = std::env::var("FB_BUILD_DATE").ok()
        .filter(|d| d.len() == 10 && d.chars().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() }))
        .unwrap_or_else(|| {
            let secs = std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|s| s.parse::<i64>().ok()).unwrap_or_else(|| {
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
            });
            let (y, m, d) = civil(secs.div_euclid(86_400));
            format!("{y:04}-{m:02}-{d:02}")
        });
    let rustc = std::env::var("RUSTC").ok()
        .and_then(|r| Command::new(r).arg("--version").output().ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().take(2).collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=FB_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=FB_BUILD_DIRTY={dirty}");
    println!("cargo:rustc-env=FB_BUILD_DATE={date}");
    println!("cargo:rustc-env=FB_BUILD_RUSTC={rustc}");
}
