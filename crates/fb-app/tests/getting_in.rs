//! The unlock and first-run screens say what they are for, why they are showing and what comes next.
use fb_app::{App, Command, Outcome, books::{GateContext, Listed}, screens};

fn gate(reason: &str) -> String {
    let mut app = App::new();
    app.host = fb_app::Host::new("0.1.0-d4", "Linux");
    let context = GateContext {
        reason: reason.into(),
        book: "/home/someone/.local/share/FreedomBooks/books/default.fbbook".into(),
        books: vec![Listed { path: "/home/someone/.local/share/FreedomBooks/books/default.fbbook".into(), kind: "main".into(), modified: 1_762_000_000_000.0, active: true, current: true }],
        home: "/home/someone".into(),
    };
    screens::render_gate_in(&app, "unlock", &context, &[], "standard").unwrap()
}

#[test]
fn the_unlock_screen_names_itself_the_version_the_reason_and_the_book() {
    let html = gate("missing");
    assert!(!html.contains("<nav"), "no sidebar");
    assert!(!html.contains("Sign in required"));
    assert!(html.contains("Books locked"));
    assert!(html.contains("Unlock books") && html.contains("Sign in</li>"), "two-step indicator");
    assert!(html.contains("<h1>Unlock your books</h1>"));
    assert!(html.contains("You’re running FreedomBooks 0.1.0-d4. Nothing is wrong with your copy."));
    assert!(html.contains("This time it doesn’t have the saved key."));
    assert!(html.contains("<code>~/.local/share/FreedomBooks/books/default.fbbook</code> · last changed"));
    assert!(html.contains("<label for=\"id_password\">Book password or recovery key</label>"));
    assert!(html.contains("Not your sign-in password."));
    assert!(html.contains("After this you’ll sign in with your <strong>user name</strong>"));
    assert!(html.contains("save the key on this computer again"));
    assert!(html.contains("<summary>I don’t have the book password or recovery key</summary>"));
    assert!(html.contains("<summary>Restore a backup instead</summary>"));
    let reasons = [
        ("unreadable", "the key saved on this computer can’t be read"),
        ("keyring", "This computer’s keyring isn’t available right now"),
        ("ask", "You’ve set FreedomBooks to ask for the book password each time it opens."),
        ("locked", "You locked FreedomBooks."),
        ("idle", "locked itself after a period of inactivity"),
    ];
    for (reason, words) in reasons {
        let html = gate(reason);
        assert!(html.contains(words), "{reason}: {html}");
        assert!(!html.contains("save the key on this computer again") || reason == "unreadable", "{reason}");
    }
    let password = gate("password");
    assert!(password.contains("book password</strong>. Enter it to open them."));
    assert!(password.contains("I don’t have the book password or recovery key"));
    let builtin = gate("builtin");
    assert!(builtin.contains("couldn’t open these books with the built-in key"));
    assert!(builtin.contains("<details class=\"gate-more\" open><summary>Restore a backup instead</summary>"));
    assert!(builtin.contains("Book password or recovery key, if protected"));
    // Unknown reasons are never echoed.
    let html = gate("<script>");
    assert!(!html.contains("<script>") && html.contains("leave the box empty"));
}

#[test]
fn first_run_says_the_name_is_the_user_name_and_shows_the_steps() {
    let html = screens::render_gate_in(&App::new(), "setup", &GateContext::default(), &[], "standard").unwrap();
    assert!(!html.contains("<nav"));
    // Worker round 1: without a book password (the default) there is no recovery-key step, so the page
    // starts at two steps; ticking the book password box adds step 3 (preload `protectionFields`).
    assert!(html.contains("Step 1 of <span id=\"setup-step-count\">2</span>"), "{html}");
    assert!(html.contains("<li id=\"recovery-step\" hidden>"), "{html}");
    assert!(!html.contains("Step 1 of 3"));
    assert!(html.contains("<label for=\"id_owner_name\">Your name</label>"));
    assert!(html.contains("This is also your <strong>user name</strong> for signing in"));
    assert!(html.contains(fb_core::backups::portable::BUILTIN_WARNING));
}

fn enroll(app: &mut App, fields: &[(&str, &str)]) -> Outcome {
    let path = fb_app::identity::SIGN_IN.to_owned();
    let token = app.tokens.mint(&path);
    let mut all = vec![("action".to_owned(), "enroll".to_owned())];
    all.extend(fields.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    app.run_renderer(Command::SubmitForm { path, token, fields: all }).unwrap()
}

#[test]
fn a_brand_new_book_shows_the_user_name_and_needs_no_existing_owner_box() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("books/default.fbbook");
    let mut app = App::new();
    app.backups.configure(d.path()).unwrap();
    app.create_protected_book(&path, "Riley Sample", "Harbor Street Bakery", Some("Synthetic book password")).unwrap();
    let html = screens::render(&mut app, fb_app::identity::SIGN_IN).unwrap().html;
    assert!(html.contains("Step 2 of 3 · Your sign-in"), "{html}");
    assert!(html.contains("User name: <strong>Riley Sample</strong>"));
    assert!(!html.contains("confirm_owner"));
    assert!(html.contains("Set password and continue"));
    let done = enroll(&mut app, &[("password", "Synthetic user password"), ("password_repeat", "Synthetic user password")]);
    assert!(matches!(done, Outcome::Redirect { ref location } if location == fb_app::protection::RECOVERY), "{done:?}");
    let html = screens::render(&mut app, fb_app::protection::RECOVERY).unwrap().html;
    assert!(html.contains("Step 3 of 3 · Save your recovery key"));
    assert!(html.contains("class=\"recovery-key\""), "first run shows the new key without asking for the book password again");

    // Reopened before the owner's sign-in was set: the explicit confirmation is still required.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("books/default.fbbook");
    let mut app = App::new();
    app.backups.configure(d.path()).unwrap();
    app.create_protected_book(&path, "Riley Sample", "Harbor Street Bakery", None).unwrap();
    app.lock();
    app.open_with_secret(&path, "").unwrap();
    let html = screens::render(&mut app, fb_app::identity::SIGN_IN).unwrap().html;
    assert!(html.contains("confirm_owner") && !html.contains("Step 2 of"), "{html}");
    let refused = enroll(&mut app, &[("password", "Synthetic user password"), ("password_repeat", "Synthetic user password")]);
    assert!(matches!(refused, Outcome::Invalid { ref html, .. } if html.contains("Confirm that you are setting up the existing owner")), "{refused:?}");
    let done = enroll(&mut app, &[("password", "Synthetic user password"), ("password_repeat", "Synthetic user password"), ("confirm_owner", "on")]);
    assert!(matches!(done, Outcome::Redirect { .. }));
}

/// After "Use this as my books" made a copy the main books while there was no saved choice, the main
/// books file stays where it is; the chooser no longer calls it the main books.
#[test]
fn the_chooser_calls_the_main_books_file_previous_once_another_book_is_the_main_one() {
    let base = "/home/someone/.local/share/FreedomBooks/books";
    let render = |copy_active: bool| {
        let context = GateContext {
            reason: "choose".into(),
            book: String::new(),
            books: vec![
                Listed { path: format!("{base}/default.fbbook"), kind: "main".into(), modified: 1_762_000_000_000.0, active: false, current: false },
                Listed { path: format!("{base}/restored-bakery/restored-bakery.fbbook"), kind: "restored".into(), modified: 1_762_000_000_000.0, active: copy_active, current: false },
            ],
            home: "/home/someone".into(),
        };
        screens::render_gate_in(&App::new(), "choose", &context, &[], "standard").unwrap()
    };
    let html = render(true);
    assert!(html.contains("Previous main books file"), "{html}");
    assert!(!html.contains(">Main books<"), "{html}");
    let html = render(false);
    assert!(html.contains("Main books") && !html.contains("Previous main books file"), "{html}");
}

/// Worker item 14 (2026-11-23): the first-run sign-in page showed "Please fill out this field" before anything was submitted.
#[test]
fn the_first_run_password_page_uses_its_own_messages() {
    let d = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.create_book(&d.path().join("book.fbbook"), &fb_app::keys::new_master_key().unwrap(), "Pat Tester", "Synthetic").unwrap();
    let path = fb_app::identity::SIGN_IN;
    let html = screens::render(&mut app, path).unwrap().html;
    assert!(html.contains("Step 2 of 2 · Your sign-in"), "{html}");
    assert!(html.contains("<form method=\"post\" class=\"narrow\" novalidate>"), "{html}");
    let token = app.tokens.mint(path);
    let out = app.run_renderer(Command::SubmitForm { path: path.into(), token, fields: vec![("action".into(), "enroll".into()), ("password".into(), String::new()), ("password_repeat".into(), String::new())] }).unwrap();
    let Outcome::Invalid { html } = out else { panic!("{out:?}") };
    assert!(html.contains("Use a user password of at least 8 characters"), "{html}");
}

#[test]
fn newer_format_gate_explains_the_upgrade_without_an_unlock_form() {
    let html = fb_app::screens::render_gate(&fb_app::App::new(), "unlock", false, "newer", &[]).unwrap();
    assert!(html.contains("These books need a newer version of FreedomBooks"));
    assert!(!html.contains("type=\"password\"") && !html.contains("data-gate=\"unlock\""));
}
