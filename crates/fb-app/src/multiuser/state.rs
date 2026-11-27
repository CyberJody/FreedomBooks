//! One person's state while their session is parked, and the exchange that swaps it into the `App`
//! for the duration of one request. Everything not listed here is the shared book host's.
//!
//! **Every field is classified, and the compiler enforces it.** [`exchange_common`] destructures
//! [`App`], [`SessionState`] and (in `swap_in`/`swap_out`) [`crate::session::Open`] without `..`, and
//! `backups::Host::exchange_personal` does the same for the Backups host. A field added to any of them
//! does not compile until it is either swapped here (per person) or listed as shared with the reason.
use crate::session::{App, Open, Tokens};
use crate::screens;
use fb_core::{CompanyId, UserId};
use std::collections::HashMap;

/// The per-person half of [`App`]. Field for field the same types, so a request runs on exactly the
/// code paths the desktop uses.
#[derive(Default)]
pub(crate) struct SessionState {
    pub(crate) identity: crate::identity::Identity,
    /// `Open.owner/owner_name/staff` while the book is open (the effective signed-in actor).
    pub(crate) owner: Option<(UserId, String, bool)>,
    pub(crate) tokens: Tokens,
    pub(crate) messages: Vec<String>,
    /// Which recurring-bill notices were already added to this person's `messages`.
    recurring_notices: std::collections::HashSet<String>,
    attachment_notes: Vec<(String, String)>,
    d7a_reviews: screens::d7a::Reviews,
    d5_reviews: screens::d5::Reviews,
    d6_reviews: screens::d6::Reviews,
    salestax_reviews: HashMap<String, fb_core::salestax_payments::Review>,
    sales_reviews: HashMap<String, fb_core::sales::Review>,
    pub(crate) frame_company: Option<CompanyId>,
    signout_token: Option<String>,
    recovery_authorized: bool,
    first_run_owner: bool,
    set_aside_note: Option<String>,
    vault_key: Option<zeroize::Zeroizing<[u8; 32]>>,
    email: crate::invoice_email::State,
    stripe: crate::invoice_stripe::State,
    ai: crate::ai::Attended,
    backups: screens::backups::Personal,
}

impl SessionState {
    pub(crate) fn starting_at(generation: u64) -> SessionState {
        SessionState { identity: crate::identity::Identity::starting_at(generation), ..Default::default() }
    }
    /// A signed-out state that refuses every sign-in with `message` (no unused generations left).
    pub(crate) fn restart_required(message: &str) -> SessionState {
        SessionState { identity: crate::identity::Identity::restart_required(message), ..Default::default() }
    }
    pub(crate) fn signed_in(&self) -> bool {
        self.identity.actor.is_some()
    }
    pub(crate) fn actor(&self) -> Option<UserId> {
        self.identity.actor
    }
    pub(crate) fn name(&self) -> Option<&str> {
        self.owner.as_ref().filter(|_| self.signed_in()).map(|(_, name, _)| name.as_str())
    }
}

/// The placeholder an unlocked-but-unsigned book carries (see `App::open_book`).
fn placeholder() -> (UserId, String, bool) {
    (UserId::new(), String::new(), false)
}

fn exchange_common(app: &mut App, s: &mut SessionState) {
    use std::mem::swap;
    // No `..` in either pattern: a new field must be classified here before this compiles.
    let App {
        // Per person: swapped.
        identity,
        recovery_authorized,
        first_run_owner,
        set_aside_note,
        email,
        stripe,
        backups,
        d7a_reviews,
        d5_reviews,
        d6_reviews,
        salestax_reviews,
        tokens,
        messages,
        recurring_notices,
        sales_reviews,
        vault_key,
        attachment_notes,
        frame_company,
        signout_token,
        // Per person, but on the shared `Open`: exchanged by `swap_in`/`swap_out` (owner, name, staff).
        open: _,
        // Shared by design.
        ai,                      // unattended capability cache is shared; only attended state is swapped below
        legacy_credentials: _,   // the open book's legacy credential-migration mode
        prepared_connection: _, // in-memory masks belong to the shared book connection
        host: _,                 // application version and OS shown in About
        protection: _,           // the book's key envelope and policy, not anyone's proof of access
        os_store_usable: _,      // this computer's keyring
        device_key: _,           // installation wrapping key, independent of who is signed in
        connection_keys: _,      // installation key status
        device_alternates: _,    // superseded installation keys
        remote_session: _,       // transport context set while a request runs
        scheduled_retry: _,      // local-profile retry delay
        book_change_pending: _,  // the host's pending change of main books
        plugins: _,              // build feature flags
        simplefin: _,
        invoice_delivery: _,
        screens: _,              // compiled templates; every render builds a fresh view model
        today: _,                // host clock
        now: _,
        net: _,                  // the fb-net process; its sign-in jobs are owned through `signin_jobs`
        signin_jobs: _,          // which session generation owns each provider sign-in job
        upgrade_notice: _,       // one notice about the book's upgrade, for the first page shown
        roster_changed: _,       // request-local event buffer, drained by the host after each request
    } = app;
    let SessionState {
        identity: s_identity,
        owner: _, // exchanged with `Open` by `swap_in`/`swap_out`
        tokens: s_tokens,
        messages: s_messages,
        recurring_notices: s_recurring_notices,
        attachment_notes: s_attachment_notes,
        d7a_reviews: s_d7a_reviews,
        d5_reviews: s_d5_reviews,
        d6_reviews: s_d6_reviews,
        salestax_reviews: s_salestax_reviews,
        sales_reviews: s_sales_reviews,
        frame_company: s_frame_company,
        signout_token: s_signout_token,
        recovery_authorized: s_recovery_authorized,
        first_run_owner: s_first_run_owner,
        set_aside_note: s_set_aside_note,
        vault_key: s_vault_key,
        email: s_email,
        stripe: s_stripe,
        ai: s_ai,
        backups: s_backups,
    } = s;
    swap(identity, s_identity);
    swap(tokens, s_tokens);
    swap(messages, s_messages);
    swap(recurring_notices, s_recurring_notices);
    swap(attachment_notes, s_attachment_notes);
    swap(d7a_reviews, s_d7a_reviews);
    swap(d5_reviews, s_d5_reviews);
    swap(d6_reviews, s_d6_reviews);
    swap(salestax_reviews, s_salestax_reviews);
    swap(sales_reviews, s_sales_reviews);
    swap(frame_company, s_frame_company);
    swap(signout_token, s_signout_token);
    swap(recovery_authorized, s_recovery_authorized);
    swap(first_run_owner, s_first_run_owner);
    swap(set_aside_note, s_set_aside_note);
    swap(vault_key, s_vault_key);
    swap(email, s_email);
    swap(stripe, s_stripe);
    swap(&mut ai.attended, s_ai);
    backups.exchange_personal(s_backups);
}

/// The open book's per-person fields. `book` is the shared connection.
fn actor_fields(open: &mut Open) -> (&mut UserId, &mut String, &mut bool) {
    let Open { book: _, owner, owner_name, staff } = open;
    (owner, owner_name, staff)
}

/// Put `s` into the app. On return `s` holds the vacant state the app had; [`swap_out`] restores it.
pub(crate) fn swap_in(app: &mut App, s: &mut SessionState) {
    exchange_common(app, s);
    // `s.identity` now holds the vacant identity; the session's owner goes onto the open book.
    let signed_in = app.identity.actor.is_some();
    if let Some(open) = app.open.as_mut() {
        let taken = s.owner.take();
        let (owner, name, staff) = taken.filter(|_| signed_in).unwrap_or_else(placeholder);
        let (o, n, st) = actor_fields(open);
        *o = owner;
        *n = name;
        *st = staff;
    }
}

/// Take the session back out of the app, leaving the vacant state and a placeholder owner.
pub(crate) fn swap_out(app: &mut App, s: &mut SessionState) {
    exchange_common(app, s);
    s.owner = None;
    if let Some(open) = app.open.as_mut() {
        let (owner, name, staff) = placeholder();
        let (o, n, st) = actor_fields(open);
        let owner = std::mem::replace(o, owner);
        let name = std::mem::replace(n, name);
        let staff = std::mem::replace(st, staff);
        if s.identity.actor == Some(owner) {
            s.owner = Some((owner, name, staff));
        }
    }
}
