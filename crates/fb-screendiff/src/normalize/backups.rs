//! DESKTOP-BACKUPS: only on the Backups page (its status grid), and only these four spans:
//! the backup folder path and the book size (different machines and engines: PostgreSQL size vs the
//! SQLCipher file), the notes after the create form (server passphrase / web-request limit vs desktop
//! key + Choose folder), and the restore part (server command vs the desktop restore-as-new-book flow).
use super::*;

pub(super) fn normalize(html: &str) -> String {
    static PAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<div class="backup-status" role="group" aria-label="Backup status">"#).unwrap());
    static FOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(<div class="backup-folder"><dt>Backup folder</dt><dd><code>)[^<]*(</code>)"#).unwrap());
    static SIZE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(<div><dt>Database size</dt><dd>)[^<]*(</dd></div>)"#).unwrap());
    static CREATE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)(<div class="backup-create"><form method="post" action="[^"]*">.*?</form>).*?(</div>\s*<section class="backup-panel">)"#).unwrap()
    });
    static RESTORE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<details class="backup-help"><summary>Restoring a backup</summary>.*?</details>|<section class="narrow backup-restore"><h2>Restore a backup</h2>.*?</section>"#).unwrap()
    });
    if PAGE.find_iter(html).count() != 1 {
        return html.to_owned();
    }
    let html = html.replace(r#"<p class="notice"><a href="/settings/book-protection/">Set up recovery and portable backups</a></p>"#, "");
    let html = html.replace("<th>Protection</th><th>Verified (UTC)</th>", "<th>Encrypted</th><th>Verified (UTC)</th>");
    let html = FOLDER.replace(&html, "${1}{backup-folder}${2}");
    let html = SIZE.replace(&html, "${1}{book-size}${2}");
    let html = CREATE.replace(&html, "${1}{desktop: create notes}${2}");
    RESTORE.replace(&html, "{desktop: restore flow}").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_four_declared_spans_normalize() {
        let django = r#"<div class="backup-status" role="group" aria-label="Backup status"><dl><div><dt>Files</dt><dd>0 files · 0 bytes</dd></div><div><dt>Database size</dt><dd>9.1 MB</dd></div><div class="backup-folder"><dt>Backup folder</dt><dd><code>/srv/b</code> · created on first backup</dd></div></dl></div>
<div class="backup-create"><form method="post" action="/c/1/backups/create/">T<button type="submit">Create backup now</button></form><p>server</p></div>
<section class="backup-panel"><h2>Backup files</h2></section>
<details class="backup-help"><summary>Restoring a backup</summary><p>server</p></details>"#;
        let rust = r#"<div class="backup-status" role="group" aria-label="Backup status"><dl><div><dt>Files</dt><dd>0 files · 0 bytes</dd></div><div><dt>Database size</dt><dd>1.0 MB</dd></div><div class="backup-folder"><dt>Backup folder</dt><dd><code>/home/x/backups</code> · created on first backup</dd></div></dl></div>
<div class="backup-create"><form method="post" action="/c/1/backups/create/">T<button type="submit">Create backup now</button></form><form method="post" action="/c/1/backups/"><button>Choose folder…</button></form><p>desktop</p></div>
<section class="backup-panel"><h2>Backup files</h2></section>
<section class="narrow backup-restore"><h2>Restore a backup</h2><form></form></section>"#;
        assert_eq!(normalize(django), normalize(rust));
        // Everything else stays literal: the file count, the create button and the folder state.
        for (from, to) in [("0 files", "1 file"), ("Create backup now", "Back up"), ("created on first backup", "not writable"), ("/backups/create/", "/backups/x/")] {
            assert_ne!(normalize(django), normalize(&rust.replace(from, to)), "{from}");
        }
        // BACKUPS-POLISH: no new rule. The desktop-only details line under a refused restore, a busy line,
        // and the Encrypted column (Unreadable vs Yes) all stay literal and still differ.
        let message = r#"<p class="errorlist" role="status">Not restored: This backup file is damaged or incomplete. Nothing was restored.</p>"#;
        let detailed = r#"<p class="errorlist" role="status">Not restored: This backup file is damaged or incomplete. Nothing was restored.<small class="message-detail">Details: x</small></p>"#;
        let with = |m: &str| rust.replacen("<div class=\"backup-status\"", &format!("{m}<div class=\"backup-status\""), 1);
        assert_ne!(normalize(&with(message)), normalize(&with(detailed)), "details line");
        assert_ne!(normalize(rust), normalize(&with(r#"<p class="notice backup-busy" role="status">Backing up…</p>"#)), "busy line");
        let row = |cell: &str| rust.replace("<h2>Backup files</h2>", &format!("<h2>Backup files</h2><table><tr><td data-label=\"Encrypted\">{cell}</td></tr></table>"));
        assert_ne!(normalize(&row("Yes")), normalize(&row("Unreadable")), "Encrypted column");
        // Not the Backups page: untouched.
        assert_eq!(normalize("<details class=\"backup-help\"><summary>Restoring a backup</summary>x</details>"), "<details class=\"backup-help\"><summary>Restoring a backup</summary>x</details>");
    }

    #[test]
    fn backups_page_rewrites_only_the_folder_path_and_engine_size() {
        let page = |folder: &str, size: &str, files: &str| format!(r#"<div class="backup-status" role="group" aria-label="Backup status"><dl><div><dt>Files</dt><dd>{files}</dd></div><div><dt>Database size</dt><dd>{size}</dd></div><div class="backup-folder"><dt>Backup folder</dt><dd><code>{folder}</code></dd></div></dl></div>"#);
        let a = normalize(&page("/srv/var/backups", "14.2\u{a0}MB", "0\u{a0}files"));
        assert_eq!(a, normalize(&page("/home/u/.local/share/FreedomBooks/backups", "1.1\u{a0}MB", "0\u{a0}files")));
        // Every other fact (here the file count) must DIFF.
        assert_ne!(a, normalize(&page("/srv/var/backups", "14.2\u{a0}MB", "1\u{a0}file")));
        // The same markup outside the Backups page is left alone.
        let other = r#"<div><dt>Database size</dt><dd>1.0 KB</dd></div>"#;
        assert_eq!(normalize(other), other);
    }

    #[test]
    fn key_notice_is_exact_and_security_words_elsewhere_must_differ() {
        let status=r#"<div class="backup-status" role="group" aria-label="Backup status"></div>"#;
        let notice=r#"<p class="notice"><a href="/settings/book-protection/">Set up recovery and portable backups</a></p>"#;
        assert_eq!(normalize(status),normalize(&format!("{notice}{status}")));
        assert_ne!(normalize(status),normalize(&format!("{}{status}",notice.replace("Set up recovery","Skip recovery"))));
        for text in ["Anyone who gets a copy of this file can open it.","Recovery key not saved yet","Show my recovery key"] {
            assert_ne!(normalize(status),normalize(&format!("<p>{text}</p>{status}")));
        }
        assert_eq!(normalize(notice),notice);
        assert_ne!(normalize(&format!("<td data-label=\"Protection\">Own password</td>{status}")),normalize(&format!("<td data-label=\"Protection\">Built-in key</td>{status}")));
    }

}
