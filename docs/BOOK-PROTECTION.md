# Book protection and recovery

The first setup screen has **Protect these books with my own password**, unchecked
by default. You can choose either mode and change it later in Settings → Book protection.

## Built-in key (the default)

Anyone who gets a copy of this file can open it.

These books open straight to Home. There is no password or recovery key to lose.
Backups open on any FreedomBooks install. File → Lock and idle lock do not apply:
there is no private password to ask for. Keep file copies where you intend others
to have access. The same warning appears during setup and in Settings and Backups.

## Your own password

Choose a password of at least eight characters. FreedomBooks generates a recovery
key and offers **Print**, **Save to USB / file…**, and **Remind me later**. The Emergency
Kit contains the company name, date, key and restore instructions. Keep it away
from this computer and separate from backups. Saving it only on this computer
does not help if the computer is lost. Anyone with the key and a backup can open it.
When you type the key, capitals, spaces and dashes do not matter, and the letters
O, I and L count as 0, 1 and 1. The last group is a check: a mistyped key is
reported as a typing mistake before FreedomBooks tries it.

Backups shows **Recovery key not saved yet** until a print or save succeeds.
**Show my recovery key** asks for your password. There is no cloud or email recovery.

**Ask for a password when opening these books** starts on. Turning it off requires
your current password, a working system keyring, and no saved service connections.
Then this computer opens directly to Home using its OS-sealed key; File → Lock and
idle lock are off. Other computers still need your password or recovery key.
A computer without a usable keyring cannot turn this setting off.

## Backups and another computer

Use the Backups screen to create a complete, verified backup. On a new install,
choose **Restore a backup**, enter a name for the new books and (for own-password
books) your password or recovery key, then choose the backup file. Within an open
book, choose **Restore a backup as a new book…**; a foreign protected backup asks:
“This backup is from another computer. Enter your password or recovery key.”
The original open book is not replaced. Bank-feed and AI connections need
reconnecting on another computer; connection keys are not in the backup.

Older backups without portable keys still restore using the original book key on
the original installation. Open them there and make a new portable backup before
moving to another computer. Older password books keep opening with their existing
password and receive a recovery key on their first unlock after this update.

## Change protection

Settings offers **Add my own password** or **Remove my own password**. FreedomBooks
takes and verifies a safety backup before changing the book key. An interrupted
change rolls back to the old book and protection setting on the next launch.
Removing protection requires your current password and acknowledgment of the
file-copy warning. Disconnect saved services first. Older backups keep their old
protection: adding a password cannot protect copies already made with the built-in key.

## Published built-in key

The shared, public key is `BUILTIN_KEY` in
`crates/fb-core/src/backups/portable.rs`: the 32 ASCII bytes
`FreedomBooks public book key v1!`. It is intentionally not a secret.
The SQLCipher CLI can open a built-in-key book with that raw key in hex:

```sql
PRAGMA key = "x'46726565646f6d426f6f6b73207075626c696320626f6f6b206b657920763121'";
SELECT name FROM ledger_company;
```

Use a backup to move complete books and attachments. A bare `.fbbook` does not
include the adjacent receipt files or portable key record. Keep those together
when copying an installation by hand.
