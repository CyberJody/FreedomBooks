//! Test builds only (feature `panic-test`, refused in optimized builds): panics on request, to prove
//! the native bridge's panic boundary. A point is armed for one route name and fires every time
//! that route is rendered or posted, like a malformed stored row would.
//!
//! - `render@<route>`: the screen's handler panics the way `v["id"].as_str().unwrap()` on a bad
//!   stored row does, with book-like text in the error value (which the log must drop).
//! - `write@<route>`: the post's first book write panics inside its transaction, after its
//!   statements ran and before COMMIT (fb-db `test_panic::BEFORE_COMMIT`).
//! - `read@<route>`: the screen's first book read panics inside its read transaction.
//! - `error-after-commit@<route>`: the post's first write commits, then reports an error.
//! - `error-before-commit@<route>` / `error-before-second-commit@<route>`: the post's first (or
//!   second) write fails before COMMIT and rolls back.
//! - `error-after-second-commit@<route>`: the post's second write commits, then reports an error.
//! - `point@<name>`: a named point in a long operation panics (a migration, a protection change's
//!   checkpoints, `backup-written`, `restore-record-saved`; see `fb_db::test_panic::point`).
//!
//! The desktop arms them from `FB_TEST_PANIC` (comma-separated) when the module loads.
use std::sync::Mutex;

static ARMED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

#[cfg(all(feature = "panic-test", not(debug_assertions)))]
compile_error!("`panic-test` is for test builds only and is refused in optimized (release) builds.");

/// Arm `point` for `route`; `point@<name>` instead arms a named point in a long operation (see
/// `fb_db::test_panic::point`: `migration`, the protection and backup checkpoints, ...).
pub fn arm(point: &str, route: &str) {
    if point == "point" {
        fb_db::test_panic::arm_point(route);
        return;
    }
    ARMED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((point.to_owned(), route.to_owned()));
}

/// The next book write panics inside its transaction, before COMMIT, whichever call makes it.
pub fn arm_next_write() {
    fb_db::test_panic::arm(fb_db::test_panic::BEFORE_COMMIT);
}

pub fn disarm() {
    ARMED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    fb_db::test_panic::disarm();
}

pub fn from_env() {
    for item in std::env::var("FB_TEST_PANIC").unwrap_or_default().split(',') {
        if let Some((point, route)) = item.trim().split_once('@') {
            arm(point, route);
        }
    }
}

fn armed(point: &str, route: &str) -> bool {
    ARMED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().any(|(p, r)| p == point && r == route)
}

pub(crate) fn on_render(route: &str) {
    if armed("read", route) {
        fb_db::test_panic::arm(fb_db::test_panic::DURING_READ);
    }
    if armed("render", route) {
        let stored = serde_json::json!({"name": "Synthetic Alice", "balance": "1234.56", "note": "password hunter2-synthetic"});
        let id: Result<&str, String> = stored["id"].as_str().ok_or_else(|| format!("row {stored} has no id"));
        let _ = id.expect("stored row shape");
    }
}

pub(crate) fn on_submit(route: &str) {
    if armed("write", route) {
        fb_db::test_panic::arm(fb_db::test_panic::BEFORE_COMMIT);
    }
    if armed("error-after-commit", route) {
        fb_db::test_panic::arm(fb_db::test_panic::ERROR_AFTER_COMMIT);
    }
    if armed("error-before-commit", route) {
        fb_db::test_panic::fail_write_before_commit(1);
    }
    if armed("error-before-second-commit", route) {
        fb_db::test_panic::fail_write_before_commit(2);
    }
    if armed("error-after-second-commit", route) {
        fb_db::test_panic::fail_write_after_commit(2);
    }
}
