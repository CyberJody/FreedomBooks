"""Clean provider display text before masking; audit counts only."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def replace(name,old,new):
    p=ROOT/name;t=p.read_text();assert t.count(old)==1,(name,old);p.write_text(t.replace(old,new))
replace('crates/fb-db/src/check_numbers.rs','''    text.chars().all(|c| {
        // All forbidden text characters also fail the shared number gate.
        char_allowed(c) || !FREE_TEXT_FORBIDDEN.iter().any(|&(lo, hi)| (lo..=hi).contains(&(c as u32)))
    })''','''    text.chars().all(free_text_char_allowed)
}

/// Same frozen Cc/Cf set for refusal of typed text and cleaning of provider text (TAB allowed).
pub fn free_text_char_allowed(c: char) -> bool {
    char_allowed(c) || !FREE_TEXT_FORBIDDEN.iter().any(|&(lo, hi)| (lo..=hi).contains(&(c as u32)))''')
p='crates/fb-core/src/bankfeeds/staging.rs'
replace(p,'''    let masked = secrets.mask(value);
    let redacted = text::redact(&masked);''','''    // Removing controls can join a credential spelling: clean BEFORE known-secret masking.
    let cleaned: String = value.chars().filter(|&c| fb_db::check_numbers::free_text_char_allowed(c)).collect();
    let masked = secrets.mask(&cleaned);
    let redacted = text::redact(&masked);''')
replace(p,'''    secrets.mask(&text::sanitize(&secrets.mask(value), limit))''','''    secrets.mask(&text::sanitize(&provider_text(secrets, value), limit))''')
replace(p,'''fn provider_pull(secrets: &SecretSet, pull: &StagedPull) -> StagedPull {''','''fn removed_characters(value: &str) -> usize {
    value.chars().filter(|&c| !fb_db::check_numbers::free_text_char_allowed(c)).count()
}
fn transaction_removed(t: &StagedTransaction) -> usize {
    removed_characters(&t.remote_id) + removed_characters(&t.description)
}
fn pull_removed(pull: &StagedPull) -> usize {
    let all = |values: &[String]| values.iter().map(|s| removed_characters(s)).sum::<usize>();
    all(&pull.skipped) + all(&pull.errors) + pull.accounts.iter().map(|a| {
        [&a.conn_key, &a.remote_id, &a.name, &a.org_name, &a.currency].into_iter().map(|s| removed_characters(s)).sum::<usize>()
            + all(&a.skipped) + a.transactions.iter().map(transaction_removed).sum::<usize>()
    }).sum::<usize>()
}
fn audit_removed(mut detail: Value, count: usize) -> Value {
    // Preserve the existing clean-provider audit contract; count only newly removed characters.
    if count > 0 { detail["removed_characters"] = json!(count); }
    detail
}
fn provider_pull(secrets: &SecretSet, pull: &StagedPull) -> StagedPull {''')
replace(p,'''        let summary = w.text(&written(secrets, &provider_text(secrets, summary), 500));''','''        let removed = removed_characters(summary);
        let summary = provider_text(secrets, summary);
        let summary = if removed > 0 { format!("{removed} control/format characters removed; {summary}") } else { summary };
        let summary = w.text(&written(secrets, &summary, 500));''')
replace(p,'''json!({"ok":false,"summary":summary,"scheduled":scheduled}))''','''audit_removed(json!({"ok":false,"summary":summary,"scheduled":scheduled}), removed))''')
replace(p,'''    let raw = pull;
    let pull = &provider_pull(secrets, pull);''','''    let raw = pull;
    let removed = pull_removed(pull);
    let pull = &provider_pull(secrets, pull);''')
replace(p,'''        let mut messages = vec![format!("{accounts} accounts, {} new transactions", counts.new)];''','''        let mut messages = vec![format!("{accounts} accounts, {} new transactions", counts.new)];
        if removed > 0 { messages.push(format!("{removed} control/format characters removed")); }''')
old='''json!({"ok":true,"scheduled":scheduled,"summary":summary,"pending_dropped_ids":dropped,"accounts":accounts,"new":counts.new,"updated":counts.updated,"posted":counts.posted,"unchanged":counts.unchanged,"skipped":skipped,"errors":errors,"pending_dropped":pending_dropped})'''
replace(p,old,'audit_removed('+old+', removed)')
replace(p,'''    let raw = transactions;
    let transactions:''','''    let raw = transactions;
    let removed = transactions.iter().map(|(a,t)| removed_characters(a) + transaction_removed(t)).sum();
    let transactions:''')
old='''json!({"staged":staged.len(),"new":counts.new,"updated":counts.updated,"posted":counts.posted,"unchanged":counts.unchanged,"transactions":staged})'''
replace(p,old,'audit_removed('+old+', removed)')
