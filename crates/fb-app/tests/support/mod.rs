//! Explicit authentication step for the existing single-owner synthetic fixtures.
//! Production never enrolls or selects a user on open; fixtures supply this individual password.
const FIXTURE_USER_PASSWORD: &str = "Individual synthetic fixture password";
pub fn sign_in(app: &mut fb_app::App) {
    if app.authenticated_session().is_ok() {
        return;
    }
    if app.bootstrap_owner().unwrap().is_some() {
        app.enroll_owner(FIXTURE_USER_PASSWORD).unwrap();
        return;
    }
    let names: Vec<String> = app
        .open
        .as_mut()
        .expect("fixture book is open")
        .book
        .read(|q| {
            q.prepare("SELECT username FROM fb_user WHERE is_active=1 AND is_staff=1")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()
        })
        .unwrap();
    assert_eq!(
        names.len(),
        1,
        "fixture must name one explicit administrator"
    );
    app.sign_in(&names[0], FIXTURE_USER_PASSWORD).unwrap();
}
