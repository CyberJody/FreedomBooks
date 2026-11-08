# People using this desktop

FreedomBooks supports one signed-in person at a time on the same desktop. It does not enable simultaneous use from other computers. Each person has an individual user name, a salted Argon2id password hash, a role, and company memberships.

Normally open FreedomBooks and sign in with your user name and user password, followed by MFA if enabled. New and restored books use the secure system keyring for background opening when available. Sign-in does not convert or rewrite book protection settings. The first open on a new computer, an unavailable keyring, or an explicitly enabled extra prompt still asks for the book password or recovery key first. The signed-in name at the top of every accounting screen is the person recorded in accounting history. Choose **Switch user / sign out** there, or from Settings, before handing the desktop to someone else. Sign-out expires every form, review, pending file chooser and recovery export. Reopen any unsaved work after signing in again. Idle time, OS lock and suspend also end the named-user session; the existing book encryption policy determines whether the file closes too.

An administrator opens **Settings → Users**, or **Switch user / sign out → Manage users**, to create a user, change a name or role, set/reset an individual password, change company memberships, or disable a user. Saving the roster signs out the current person and revokes pending authority. Disabled people remain in history. Set each person's own password; FreedomBooks supplies no default password.

| Role | Allowed work |
| --- | --- |
| Administrator | Accounting work in their member companies; manage the local book's users and memberships, company administration, backups, recovery and book protection. |
| Bookkeeper | Read, print and record accounting work in their member companies, subject to the existing accounting rules. |
| Read-only | Read, print and export accounting information in their member companies. Saving changes is refused by the application and native paths. |

Roles apply across the book; membership limits ordinary accounting access to specified companies, including for administrators. An administrator can manage the book's roster and grant membership. Keep an active administrator with a configured password for every company. FreedomBooks refuses changes that remove the last one. A person who has forgotten a user password needs another administrator to reset it. Keep a second administrator if this recovery path is important to your business.

## Existing owner and upgrades

Opening an older book uses the existing verified pre-upgrade backup flow before migration 0185. Previous migrations, owner identity, companies, balances and accounting history are retained. A book with no configured individual passwords offers explicit setup for its retained owner (or its sole active staff operator for legacy desktop books without an owner mapping). Confirm that you are setting up that owner and choose a new individual password. This enrollment is atomic. If setup is interrupted, reopen the book and finish it; no default password or automatic authentication is created. Once enrolled, every reopen requires explicit sign-in.

An ambiguous legacy book with several staff operators and no retained owner cannot silently choose one; it requires deliberate recovery outside this first desktop workflow. There is no hidden reset password, first-user fallback, or recovery-key impersonation path.

## Book unlock and individual sign-in

These are separate controls with separate passwords:

- **Book password / recovery key:** unlocks the stored file under the existing encryption and recovery protocol. The secure OS keyring normally opens the encrypted book internally on this computer. This grants no user session. An administrator can enable an extra book-password prompt in Book protection; that explicit preference is retained. If the keyring is missing or its saved key cannot be read, use the book password or recovery key to reconnect the book on this computer.
- **Individual user password:** authenticates a person to the running application, controls their permissions, and attributes their accounting work. It does not encrypt or rekey the book.

Backups retain their portable book-password and recovery-key envelope, named users, and MFA settings. A restore does not depend on the original OS keyring and never signs in a person automatically. The destination computer seals its own keyring copy after the backup has been unlocked with the book password or recovery key; named-user sign-in and MFA are still required. Saved bank connections remain sealed to their device and can require reconnecting on another computer.

Built-in-key mode remains built-in-key mode: anyone with a copy of that file can open it. Adding user passwords does not give that file password protection. A person with access to a book's encryption key and suitable external tools can access its data; named-user sign-in is the application authorization boundary. A book recovery key does not reset or replace an individual's password. Retain the existing backups, book password and recovery kit separately.

## Integration contract

`fb_app::identity` owns the shared identity and authorization hooks:

- `App::authenticated_session(&mut self)` returns the verified `actor: UserId`, `generation: u64` and `role: Role`. It checks enrollment, current active status and the live role against the open book. Unlocking alone gives no authenticated actor.
- `App::authorize(company: Option<CompanyId>, Permission::{Read, Write, Administer})` makes an explicit permission decision. Supply `Some(company)` for every company operation. Administrators still need membership for that company's ordinary accounting data.
- Capture `App::session_generation()` when accepting deferred/agent work. Under the existing APP mutex, immediately before execution, call `App::require_session(generation, Some(company), permission)`. Also bind the accepting actor and company in the transport's own grant. Recheck before delivering a deferred result. A stale generation always fails; it never acquires the next person's authority.
- Route writes through `App::run` and existing application/core services. App command/query and screen boundaries enforce identity, membership and permissions. Do not accept a caller-selected actor or bypass the boundary by taking a raw mutable book. `open_mut()` requires a live named session; its `Open.owner`, `owner_name` and `staff` retain the effective signed-in actor semantics. `staff` means administrator for desktop screens. An unlocked-but-unsigned `Open.owner` is a placeholder and grants no authority.
- Sign-out, switching, lock, reopen and successful roster changes advance the generation and invalidate forms, reviews, dialog tickets and recovery capabilities. Native asynchronous work captures that generation at acceptance, validates under the APP lease, and validates its result again before delivery.

The identity layer restores file-wide credential masks internally before the first Writer, then performs company-specific credential maintenance only under the verified person's company memberships. It exposes no connection secret to the renderer or another company's user. Explicit first-owner enrollment restores these masks before committing its hash so old books with connections can enroll safely.

## Optional multi-factor authentication

Open **Settings → Multi-factor authentication**, or the link on **Your desktop session**. MFA is per person, per book, and starts disabled. Confirm your user password, choose **Set up authenticator**, and scan the QR code with your authenticator app. If scanning is unavailable, expand **Can’t scan? Enter a setup key manually** and add a time-based account (six digits, 30 seconds). Enter a generated code to enable it. Unconfirmed setup expires after five minutes and does not change sign-in.

Save the ten recovery codes somewhere safe outside the book. Each recovery code works once, together with your user password. Choose **I saved my codes** to clear the displayed copy. Recovery codes cannot be displayed again after that session. Replacing the authenticator or generating new recovery codes invalidates all prior recovery codes.

Once enabled, password verification leads to a separate code screen. Until it succeeds, book operations, exports, and AI/API/MCP grants are unavailable. Codes are accepted with at most one 30-second interval of clock skew and cannot be reused. Wait for the next code if you just used the current one. A pending sign-in expires after five minutes. Five unsuccessful password/code attempts impose a one-minute in-process delay; this is not an operating-system-level lockout.

Changing or disabling MFA requires your password and a current authenticator code or unused recovery code. MFA changes invalidate existing AI connections and pending forms. A user-password reset does not remove MFA. Each role, including read-only, can manage its own MFA; this interface does not let administrators bypass another person's factor. If both the authenticator and every recovery code are lost, this version has no MFA bypass.

MFA protects application sign-in. It does not encrypt the stored file or defend against someone controlling your OS account or possessing the book's encryption key. Choose your own password in **Book protection** to protect the book file; built-in-key mode does not provide that password protection. The authenticator secret is stored inside the SQLCipher book, and only hashes of recovery codes are stored. Audit records contain the actor and the type of MFA event, never codes or setup secrets.

Backups retain the MFA configuration at the time of the backup. Restoring an old backup can restore old authenticator settings and previously unused recovery codes; generate a fresh recovery set after restoring. The book recovery key and MFA recovery codes serve different purposes. YubiKey support is deferred; no hardware key is required for this version.

Implementation uses [RFC 6238 TOTP](https://www.rfc-editor.org/rfc/rfc6238.html), checked against its published test vectors. No authenticator vendor account or network request is needed by FreedomBooks.
