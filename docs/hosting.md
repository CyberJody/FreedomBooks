# Host these books

One computer keeps the books. While **hosting** is on, other computers in your own network or
tailnet can use those books through it: each person signs in with their own user name and password,
and everyone works in the same books at once. The other computers never get a copy of the book file.

Hosting is **off** until an Administrator turns it on. Only an Administrator who is signed in on the
host computer can turn it on or off, pair a computer or revoke one.

## Before you start

- The host computer must stay on, with FreedomBooks open and the books unlocked, while others work.
- Every person who connects needs their own user (Settings › Users). The roles are the usual ones:
  Administrator, Bookkeeper and Read-only.
- The computers must be on the same home or office network, or on the same
  [Tailscale](https://tailscale.com/) tailnet. FreedomBooks never listens on the internet, never
  opens a port on your router and does not use UPnP. If you are not sure the other computer can reach
  the host, put both on Tailscale.

## Turn hosting on

1. On the host computer, sign in as an Administrator.
2. Choose **Settings › Host these books…**.
3. Under **Listen on**, choose one of this computer's addresses. Only private addresses are offered:
   your local network (for example `192.168.1.20`) or Tailscale (`100.x.y.z` or `fd7a:…`).
   FreedomBooks never offers "all addresses" or a public address. If an address is in the Tailscale
   range but no Tailscale connection owns it, the screen warns you: such an address may be your
   internet provider's shared address, not your tailnet.
4. Keep the suggested port (47100) unless something else uses it, and choose **Turn on hosting**.
5. The screen shows **the address to give the other computers**, for example `192.168.1.20:47100`,
   and this computer's fingerprint.

Hosting stays on until an Administrator stops it. If you quit FreedomBooks or close the books,
hosting stops and the people connected are told "The host stopped sharing these books."; it starts
again by itself the next time the books are opened on this computer, even before anyone signs in
at the host. While hosting is on, the window title says so on every screen, with how many computers
are connected (for example "Hosting on — 2 computers connected").

If the chosen address leaves this computer (you unplug the network or leave the Wi-Fi) or its
network changes, hosting stops and the Hosting screen says why; choose an address again. FreedomBooks
cannot tell a different network that happens to use the same numbers (a laptop host that wakes up on
another Wi-Fi with the same address keeps listening there; strangers still cannot pair or connect
without a code). **A host that travels should listen on its Tailscale address**, which stays with
the computer.

## Pair a computer

Pairing is done once per computer.

1. On the host's Hosting screen, choose **Pair a computer…**. A one-time code appears, for example
   `K7QM-2XDP-9RWA-4HTE`. It works once, for ten minutes.
2. On the other computer, choose **File › Connect to a FreedomBooks host…** and enter the host
   address, the code and a name for that computer (for example "Front desk laptop").
3. The other computer connects and shows the host's sign-in screen. Sign in with your own user name
   and password (and your second factor, if you use one).

The code proves to both computers that they are talking to each other, not to something in
between; after pairing each computer recognises the other by its certificate. A wrong or expired
code is refused with one plain message. If someone sends too many wrong codes, that code stops
working for them.

## Paired computers

The Hosting screen lists every paired computer with when it was paired, by whom, when it was last
seen and whether it is connected now. The list refreshes automatically. “Now” means a computer
is connected. Times show the host computer’s time zone. The remote session count includes sessions
kept briefly for a reconnect. An administrator at the host can open **Settings › Users** to see
who is signed in and which computer each person is using.

To **revoke** a computer (a lost laptop, someone leaving), choose **Revoke** next to it and confirm. It is cut off
at once and can only come back by pairing again with a new code. You can revoke a computer while
hosting is off too. The revocation is kept in the books themselves and written into the list of
paired computers and the audit log (with your name) at once, so neither putting back an older copy
of the list nor restoring a backup of the books made before the revocation lets it in again.
If the screen ever shows **Not saved** next to a revoked computer (for example the disk was full),
the computer is already refused, but keep the screen open and fix the disk: the warning goes away once
the revocation is saved.

## While hosting

- **Locking the host computer** (File › Lock, the idle lock, or the computer sleeping or locking its
  screen) signs out **only the person at the host**, at once, even while a connected person's work is
  running. The books stay open for connected people and for [AI approvals configured to work while
  locked](local-ai.md#choose-when-the-connection-can-work). Attended AI connections resume when the
  approving person signs in at the host and opens the approved company.
- **For books protected by a book password, Lock does not ask for that password again while
  hosting.** The books stay open, so the book's key stays in this computer's memory until hosting
  stops or FreedomBooks quits; after Lock, a user's own password is what is asked. Quitting (or
  stopping hosting and then locking) is what closes the books. While hosting, the host computer's own
  screen lock and disk encryption matter more.
- **Closing the books or quitting** on the host stops hosting for everyone and pauses unattended AI
  access. Unattended approvals resume when the same books reopen, including when hosting resumes
  after a restart.
- **Backups, restores and changing the book password or key** can only be done at the host computer.
  They go ahead even when others are signed in; those people are signed out and told why.
- A connected person who does nothing for 15 minutes is signed out. Locking a connected computer
  also signs out only that person and brings back this host's sign-in screen. If the host is
  temporarily unreachable, the connected computer waits on a locked screen and reconnects
  automatically using its saved pairing.
- Some things can only be done at the host computer: unlocking or restoring books, book protection,
  the recovery key and Emergency Kit, backups, attachments, AI connections, and connecting bank
  feeds, Gmail or Stripe. Sync bank feeds at the host too. Connected bookkeepers can review the
  downloaded transactions and acknowledge missing dates after checking the bank statement.

## Audit log

The host keeps a separate remote-access audit log: pairing codes made and used, computers revoked,
connections, sign-ins and sign-outs, each with the computer's name and the person. Every entry is
chained and sealed with a key kept inside the books, so editing it, removing entries or putting back
an older copy is detected. The Hosting screen shows how many entries it has.

If the log cannot be written (for example the disk is full), remote access pauses at once, everyone
connected is signed out, and the Hosting screen says why. Fix the cause, then choose **Restart
hosting**: the log is checked and repaired, and the repair is recorded.

If hosting refuses to start because the log or the list of paired computers is not the one the books
recorded (an older copy was put back, a file was edited, or the key was lost), the Hosting screen says
so and offers the repair: the old file is **set aside** (kept, renamed with the date) and a new one is
started. Setting the list of paired computers aside means every computer pairs again. Each repair is
recorded with the Administrator's name.

## Restoring a backup

The host's certificate and keys are kept inside the encrypted books, so they travel with every
backup. A restored backup comes back with **hosting off**, even if hosting was on when the backup was
made: an Administrator turns it on again on the computer where the restored books are used. Paired
computers stay paired (they recognise the host's certificate), and computers revoked since the backup
was made stay revoked (the list of paired computers on this computer is not in the backup).

Anyone who has a backup and its password has the books, and with them the host's identity: they
could pretend to be the host to paired computers on the same network. Keep backups as safe as the
books.

## Limits

- **Your own network only.** There is no internet access to the host. Use Tailscale to reach it from
  elsewhere.
- **A crowded network neighbour.** On IPv6, a computer in the same local network prefix (the same
  /64) as a paired computer could, by opening connections very quickly, delay that computer's
  connections. This is accepted for your own network; it does not let anyone in.
- **The host's books are as safe as the host computer.** Anyone who can use the host computer while
  the books are unlocked can use them. The books themselves are the anchor of the checks above: if
  someone with access to your user account on the host puts back an older copy of the book file
  *together with* older copies of the audit log and the list of paired computers, everything agrees
  and a computer revoked in between could connect again. That also rolls back the accounting, which
  you would notice; a restore through FreedomBooks never does it.
- **The host must be this desktop app.** A host that runs as a service without a desktop is not
  available yet.
- Attachments cannot be added from a connected computer yet.

## Licence note (not legal advice)

FreedomBooks is licensed AGPL-3.0-or-later. Section 13 of that licence asks that people who use a
*modified* version over a network be offered its source. If you only run FreedomBooks as released,
nothing changes. If you modify the host and let others use it over the network, check what section 13
asks of you. This note is a flag, not legal advice.
