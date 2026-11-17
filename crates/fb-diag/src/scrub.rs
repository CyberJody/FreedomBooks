//! What the problem log may hold. The rule is simple: **no free text**. Messages from panics, from
//! Rust errors and from JavaScript exceptions can carry anything (a customer's name, an amount, a
//! memo, part of a key, a path, SQL with its values), and no scrubber is reliably right about
//! them, so they are never written. What a developer needs to find the code that failed is kept:
//!
//! - **where**: a source location (`crates/fb-app/src/x.rs:12:5`) and code-only stack frames
//!   (symbols of this source tree's crates, and the app's own JavaScript files);
//! - **what kind**: a code-owned identifier (`panic`, `error:Render`, `caught:TypeError:ENOENT`)
//!   and, where it helps, a fixed phrase chosen by the code from a table, never derived from the
//!   value (see [`crate::panic::category`]);
//! - **during what**: the bridge call or main-process step, and the screen's route name;
//! - **which build**: version, commit, schema count.
//!
//! The functions here reduce code coordinates and check identifiers; each one keeps a small
//! alphabet and a length cap, so even a coordinate that somehow held other text stays harmless.

/// A code-owned identifier (operation, route name, error kind): letters, digits and `:_.-` only.
pub fn ident(raw: &str) -> String {
    let kept: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || ":_.-".contains(c) { c } else { '_' }).collect();
    let kept = cap(&kept, 80);
    if kept.is_empty() { "unknown".into() } else { kept }
}

/// Code coordinates already reduced by [`location`], [`backtrace`] or [`js_frame`]: a character
/// allowlist and a cap, applied again when an entry is written.
pub fn code(text: &str, limit: usize) -> String {
    let kept: String = text.chars().map(|c| if c.is_ascii_alphanumeric() || "_:<>{}()[]&*,. #'-=!/+$".contains(c) { c } else { '_' }).collect();
    cap(&kept, limit)
}

fn cap(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A source file path from a panic location or backtrace, reduced to code coordinates:
/// `crates/fb-app/src/x.rs`, `serde_json-1.0.140/src/value/index.rs`, `library/core/src/option.rs`.
/// Any other absolute path keeps only its file name.
pub fn path(raw: &str) -> String {
    let p = raw.replace('\\', "/");
    let tail = if let Some(i) = p.find("/registry/src/") {
        // `<index>/<crate>-<version>/...`: drop the index directory.
        let rest = &p[i + "/registry/src/".len()..];
        rest.split_once('/').map_or(rest, |(_, r)| r).to_owned()
    } else if let Some(i) = p.find("/git/checkouts/") {
        let rest = &p[i + "/git/checkouts/".len()..];
        rest.split_once('/').map_or(rest, |(_, r)| r).to_owned()
    } else if let Some(i) = p.find("/rustc/") {
        let rest = &p[i + "/rustc/".len()..];
        rest.split_once('/').map_or(rest, |(_, r)| r).to_owned()
    } else if let Some(i) = p.rfind("/crates/") {
        p[i + 1..].to_owned()
    } else if p.starts_with('/') || p.get(1..3) == Some(":/") {
        p.rsplit('/').next().unwrap_or("").to_owned()
    } else {
        p.clone()
    };
    let kept: String = tail.chars().map(|c| if c.is_ascii_alphanumeric() || "_./-+".contains(c) { c } else { '_' }).collect();
    cap(kept.trim_start_matches("./"), 160)
}

/// `file:line:column` with the file reduced by [`path`].
pub fn location(file: &str, line: u32, column: u32) -> String {
    format!("{}:{line}:{column}", path(file))
}

/// Frames of a `std::backtrace::Backtrace` (its `Display` text) in this source tree's crates:
/// `symbol at crates/...:line:col`, or the bare symbol when the build has no line tables.
/// Addresses, symbol hashes, dependency and standard-library frames and the panic hook's own
/// frames are left out; at most 48.
pub fn backtrace(text: &str) -> Vec<String> {
    let mut frames: Vec<(String, Option<String>)> = vec![];
    for line in text.lines() {
        let t = line.trim();
        if let Some(at) = t.strip_prefix("at ") {
            if let Some(last) = frames.last_mut()
                && last.1.is_none()
            {
                last.1 = Some(code_location(at));
            }
            continue;
        }
        let Some((n, symbol)) = t.split_once(": ") else { continue };
        if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        frames.push((symbol_name(symbol), None));
    }
    // Our crates' symbols (`fb_app::...`, `<fb_app::X as ...>::...`), minus the hook's own.
    let krate = |symbol: &str| -> Option<String> {
        let s = symbol.strip_prefix('<').unwrap_or(symbol);
        let name = s.split("::").next()?;
        (name.starts_with("fb_") && !s.starts_with("fb_diag::panic::")).then(|| name.to_owned())
    };
    let hook = |at: &str| at.starts_with("crates/fb-diag/src/panic.rs");
    frames
        .into_iter()
        .filter_map(|(symbol, at)| {
            // Line tables name the file from the workspace (`crates/fb-app/src/x.rs`) or from the
            // crate (`src/x.rs`); an inlined frame may carry only a short symbol (`render`) with
            // its workspace file. Frames of other code, and the hook's own, are dropped.
            let at = match (krate(&symbol), at) {
                (_, Some(at)) if hook(&at) => return None,
                (_, Some(at)) if at.starts_with("crates/") => Some(at),
                (Some(k), Some(at)) if at.starts_with("src/") => Some(format!("crates/{}/{at}", k.replace('_', "-"))),
                (Some(_), _) => None,
                (None, _) => return None,
            };
            Some(match at {
                Some(at) => format!("{symbol} at {at}"),
                None => symbol,
            })
        })
        .take(48)
        .collect()
}

fn code_location(at: &str) -> String {
    // `file:line:col`; the file may itself contain ':' on Windows.
    let mut parts = at.rsplitn(3, ':');
    let (col, line, file) = (parts.next(), parts.next(), parts.next());
    match (file, line.and_then(|l| l.parse::<u32>().ok()), col.and_then(|c| c.parse::<u32>().ok())) {
        (Some(file), Some(line), Some(col)) => location(file, line, col),
        _ => path(at),
    }
}

fn symbol_name(raw: &str) -> String {
    // `fb_app::screens::render::h0123456789abcdef`: the hash says nothing useful.
    let s = match raw.rsplit_once("::h") {
        Some((head, hash)) if hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()) => head,
        _ => raw,
    };
    let kept: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || "_:<>{}()[]&*,. #'-=!".contains(c) { c } else { '_' }).collect();
    cap(&kept, 200)
}

/// One frame of a main-process JavaScript stack, as the shell extracted it from the engine's
/// frames (see `electron/main/diagnostics.cjs`): the file, relative to the app (`main/main.cjs`),
/// and the line and column. Accepted only when the file has the shape of one of the app's own
/// (`main/` or `preload/`, a lowercase name of letters and dashes); anything else is dropped
/// whole. Function names are not kept: they add nothing the line does not say, and a name is
/// one more place text could hide.
pub fn js_frame(file: &str, line: u64, column: u64) -> Option<String> {
    let mut parts = file.split('/');
    let (Some(folder), Some(name), None) = (parts.next(), parts.next(), parts.next()) else { return None };
    if !["main", "preload"].contains(&folder) {
        return None;
    }
    let (stem, ext) = name.rsplit_once('.')?;
    if !["cjs", "js", "mjs"].contains(&ext) || stem.is_empty() || stem.len() > 30 || !stem.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
        return None;
    }
    if line == 0 || line > 1_000_000 || column > 1_000_000 {
        return None;
    }
    Some(format!("{folder}/{name}:{line}:{column}"))
}

/// A JavaScript error's constructor name: one of the engine's and Node's own, else `other`.
pub fn js_error_name(name: &str) -> &'static str {
    const NAMES: &[&str] = &["Error", "TypeError", "RangeError", "ReferenceError", "SyntaxError", "EvalError", "URIError", "AggregateError", "SystemError", "AbortError", "DOMException", "string", "number", "object", "undefined", "boolean", "symbol", "bigint", "function"];
    NAMES.iter().find(|n| **n == name).copied().unwrap_or("other")
}

/// A JavaScript error's `code`: Node's `ERR_*` codes, the system's `E*` error names, fb-node's own
/// `FB_*` codes and Node-API statuses keep their shape; anything else is `other`.
pub fn js_error_code(code: &str) -> String {
    const STATUS: &[&str] = &[
        "GenericFailure", "InvalidArg", "ObjectExpected", "StringExpected", "NameExpected", "FunctionExpected", "NumberExpected", "BooleanExpected",
        "ArrayExpected", "PendingException", "Cancelled", "EscapeCalledTwice", "HandleScopeMismatch", "CallbackScopeMismatch", "QueueFull", "Closing",
        "BigintExpected", "DateExpected", "ArrayBufferExpected", "DetachableArraybufferExpected", "WouldDeadlock", "SESSION_CHANGED", "BUSY", "LOCKED",
    ];
    const ERRNO: &[&str] = &[
        "E2BIG", "EACCES", "EADDRINUSE", "EADDRNOTAVAIL", "EAGAIN", "EAI_AGAIN", "EAI_NONAME", "EBADF", "EBUSY", "ECANCELED", "ECONNABORTED",
        "ECONNREFUSED", "ECONNRESET", "EDQUOT", "EEXIST", "EFAULT", "EFBIG", "EHOSTUNREACH", "EINTR", "EINVAL", "EIO", "EISDIR", "ELOOP", "EMFILE",
        "EMLINK", "ENAMETOOLONG", "ENETDOWN", "ENETUNREACH", "ENFILE", "ENODEV", "ENOENT", "ENOMEM", "ENOSPC", "ENOSYS", "ENOTCONN", "ENOTDIR",
        "ENOTEMPTY", "ENOTSUP", "ENXIO", "EOF", "EPERM", "EPIPE", "EPROTO", "EROFS", "ESPIPE", "ESRCH", "ETIMEDOUT", "ETXTBSY", "EXDEV",
    ];
    if let Some(known) = STATUS.iter().chain(ERRNO).find(|k| **k == code) {
        return (*known).to_owned();
    }
    // Shaped codes are kept by shape only up to a fixed form: `ERR_` / `FB_` words of capitals
    // joined by underscores (`ERR_INVALID_ARG_TYPE`, `FB_BOOK_WRONG_KEY`). Digits are not allowed,
    // so no key or secret can pass as one.
    let shaped = |prefix: &str| {
        code.strip_prefix(prefix).is_some_and(|rest| {
            code.len() <= 48 && !rest.is_empty() && rest.split('_').all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_uppercase()))
        })
    };
    if shaped("ERR_") || shaped("FB_") { code.to_owned() } else { "other".to_owned() }
}

/// The operating system's name and version as the shell reports it (`Debian GNU/Linux 13
/// (trixie)`): ASCII letters, digits, spaces and `._()/+-` only, at most 60 characters.
pub fn os_name(raw: &str) -> String {
    let kept: String = raw.chars().filter(|c| c.is_ascii_alphanumeric() || " ._()/+-".contains(*c)).collect();
    cap(kept.split_whitespace().collect::<Vec<_>>().join(" ").as_str(), 60)
}

/// A folder for showing to the owner: the home folder as `~`.
pub fn shown_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    match std::env::var_os("HOME").map(std::path::PathBuf::from) {
        Some(home) if !home.as_os_str().is_empty() => match path.strip_prefix(&home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => text,
        },
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_keep_code_coordinates_only() {
        assert_eq!(path("crates/fb-app/src/screens/d6.rs"), "crates/fb-app/src/screens/d6.rs");
        assert_eq!(path("/home/someone/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde_json-1.0.140/src/value/index.rs"), "serde_json-1.0.140/src/value/index.rs");
        assert_eq!(path("/rustc/4eb161250e340c8f48f66e2b929ef4a5bed7c181/library/core/src/option.rs"), "library/core/src/option.rs");
        assert_eq!(path("/home/someone/work/FreedomBooks/crates/fb-db/src/lib.rs"), "crates/fb-db/src/lib.rs");
        assert_eq!(path("/home/someone/private/thing.rs"), "thing.rs");
        assert_eq!(location("./crates/fb-node/src/lib.rs", 12, 5), "crates/fb-node/src/lib.rs:12:5");
    }

    #[test]
    fn backtraces_keep_this_source_trees_frames_only() {
        let text = "   0: std::backtrace::Backtrace::force_capture\n             at /rustc/abc/library/std/src/backtrace.rs:331:13\n   1: fb_diag::panic::Raw::take::h0123456789abcdef\n             at /home/someone/src/FreedomBooks/crates/fb-diag/src/panic.rs:60:9\n   2: fb_app::screens::render::h0123456789abcdef\n             at /home/someone/src/FreedomBooks/crates/fb-app/src/screens.rs:150:9\n   3: serde_json::de::from_str\n             at /home/someone/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde_json-1.0.140/src/de.rs:1:1\n   4: fb_node::render\n   5: core::ops::function::FnOnce::call_once\n   6: fb_diag::tests::case::{{closure}}\n             at ./src/tests.rs:7:3\n   7: do_call<fb_app::X>\n             at /rustc/abc/library/std/src/panicking.rs:1:1\n   8: on_render\n             at /home/someone/src/FreedomBooks/crates/fb-app/src/test_panic.rs:57:20\n   9: take\n             at /home/someone/src/FreedomBooks/crates/fb-diag/src/panic.rs:60:9\n";
        assert_eq!(backtrace(text), vec!["fb_app::screens::render at crates/fb-app/src/screens.rs:150:9".to_owned(), "fb_node::render".to_owned(), "fb_diag::tests::case::{{closure}} at crates/fb-diag/src/tests.rs:7:3".to_owned(), "on_render at crates/fb-app/src/test_panic.rs:57:20".to_owned()]);
    }

    #[test]
    fn javascript_frames_have_the_engines_shape_and_the_apps_own_files() {
        assert_eq!(js_frame("main/main.cjs", 216, 31).as_deref(), Some("main/main.cjs:216:31"));
        assert_eq!(js_frame("preload/preload.cjs", 9, 2).as_deref(), Some("preload/preload.cjs:9:2"));
        for file in ["/home/someone/app/main/main.cjs", "node_modules/x/y.js", "main/Synthetic Alice.cjs", "main/hunter2-synthetic.cjs", "main/x.txt", "../main.cjs", "main/sub/x.cjs", "scripts/main.cjs", "main/.cjs"] {
            assert_eq!(js_frame(file, 1, 1), None, "{file}");
        }
        assert_eq!(js_frame("main/main.cjs", 0, 1), None);
    }

    #[test]
    fn javascript_names_and_codes_come_from_fixed_lists() {
        assert_eq!(js_error_name("TypeError"), "TypeError");
        assert_eq!(js_error_name("Synthetic Alice"), "other");
        assert_eq!(js_error_code("ENOENT"), "ENOENT");
        assert_eq!(js_error_code("GenericFailure"), "GenericFailure");
        assert_eq!(js_error_code("ERR_INVALID_ARG_TYPE"), "ERR_INVALID_ARG_TYPE");
        assert_eq!(js_error_code("FB_BOOK_WRONG_KEY"), "FB_BOOK_WRONG_KEY");
        for secret in ["JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP", "JBSWYDPEHPKPXP", "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6", "hunter2-synthetic", "ERR_1234", "FB_A1"] {
            assert_eq!(js_error_code(secret), "other", "{secret}");
        }
    }

    #[test]
    fn identifiers_are_a_small_alphabet() {
        assert_eq!(ident("invoices:detail"), "invoices:detail");
        assert_eq!(ident("x y/z\u{202e}"), "x_y_z_");
        assert_eq!(ident(""), "unknown");
        assert_eq!(os_name("Debian GNU/Linux 13 (trixie)\u{202e}<b>"), "Debian GNU/Linux 13 (trixie)b");
    }
}
