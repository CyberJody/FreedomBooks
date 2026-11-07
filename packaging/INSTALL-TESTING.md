# Real-machine installer checklist

Use a spare local Windows account or Linux account and synthetic books only.
Record the operating system version, installer version, checksum, result and
screenshots. The alpha is unsigned. No Docker, terminal, Rust, Node, Python,
database server or separate runtime installation is part of the customer path.

## Windows 10 and Windows 11 (x64), standard user

- Download the single `FreedomBooks-Setup-<version>.exe` file.
- Double-click it. If Windows shows its unsigned-app warning, select **More
  info**, check the filename, then **Run anyway** for this known test build.
  If organizational policy blocks it, record that result; do not change policy.
- Confirm installation needs no administrator password or UAC elevation.
  This is a one-click installer, so it has no Next/Finish wizard pages. It should
  install for the current user and open FreedomBooks.
- Confirm the name and icon are FreedomBooks, including the taskbar. Confirm a
  desktop shortcut and a Start menu shortcut exist and both open the same app.
- On **Set up your books**, enter a fictitious owner and company. Leave
  **Protect these books with my own password** unchecked and confirm the note
  "Anyone who gets a copy of this file can open it." Select **Create books**.
  Confirm Home appears, readable at 100%, 125%, 150% and 200% display scaling,
  with no missing icons.
- Confirm **File › Lock** is unavailable, and **Settings › Book protection**
  says why.
- Close the window. Open it again from Start. Confirm it opens straight to the
  same company's Home with no password.
- With the app open, select the other shortcut. Confirm one window is focused,
  with no duplicate instance or locked-book error.
- Check that installation did not ask to download a Visual C++ redistributable
  or any other prerequisite. Repeat on a clean offline account/machine after
  copying the installer there.
- Close the app. Double-click the same installer again, then a newer test
  installer. Confirm reinstall/upgrade keeps the synthetic company and password.
- Open **Settings > Apps**, choose **FreedomBooks**, then **Uninstall**. Confirm
  the uninstaller needs no administrator password and removes application files,
  shortcuts and the Apps entry. Synthetic books must remain in the user's app
  data folder; reinstall and confirm the same company reopens with its password.
- In the app's installed folder, check `licenses/electron/LICENSE`,
  `licenses/electron/LICENSES.chromium.html`, and `THIRD-PARTY-NOTICES.md`.
  A publication build must also contain `licenses/freedombooks/LICENSE`.

These Windows results must be recorded by the lead or Jody. A Linux cross-build,
PE inspection, NSIS compilation or fuse readback does not prove any Windows UI,
DPAPI/keyring, ACL, shortcut, SmartScreen, upgrade or uninstaller behavior.

## Linux Mint 22.x (x64) and Debian 13

- Download the single `FreedomBooks-<version>-amd64.deb` file.
- Double-click it, choose **Install Package** in the system package installer,
  and authorize the normal system package installation if asked. The package
  manager must resolve the desktop libraries itself. No separate prerequisite
  instructions should appear. A network connection may be needed by apt.
- Find **FreedomBooks** in the menu under Office/Finance. Confirm the name/icon
  and open it. Follow the first-run, Lock, wrong password, unlock, close/reopen
  and second-shortcut checks above, using a synthetic company.
- Repeat with the machine's ordinary keyring locked or unavailable. The setup
  page must require a password and still open the book; never accept an
  unprotected plaintext-key fallback.
- Install the same package again and then a newer test package through the
  package installer. Confirm books/password survive. Use the system Software
  Manager to remove FreedomBooks, reinstall it, and confirm the book remains.
- Repeat first launch under the default Mint security/AppArmor settings. Do not
  disable the app sandbox or system policy to make a machine test pass.
- Confirm books and keys are under `~/.local/share/FreedomBooks` (or the account's
  `XDG_DATA_HOME/FreedomBooks`). Upgrade a synthetic book from the earlier
  configuration-directory version: both the book and its key must move, retain
  their hashes, and unlock with the same password. Settings remain in `.config`.
- On Debian with user namespaces enabled, confirm the sandbox helper is 0755.
  On Mint with AppArmor's userns restriction enabled, confirm the app-specific
  AppArmor profile is loaded and the helper stays 0755. Check removal deletes
  the profile, and reinstall recreates it when required.
- For the portable option, download the single `.AppImage`. In the file manager,
  open **Properties > Permissions** and allow it to run as a program if the
  browser did not preserve its executable permission. Double-click it. Confirm
  no FUSE package, other runtime, terminal command or admin password is needed.
  In the same account, confirm it opens the locked company created by the `.deb`.
  For a separate fresh first-run check, use another new desktop account. Repeat
  restart/lock checks. AppImage is portable; a Start/menu
  entry and an uninstaller are supplied by the `.deb` option instead.
- Check the same notices in the installed folder listed above.

Record unsigned-app warnings and every extra click. In particular, verify the
AppImage starts by double-click on Mint: the headless extract-and-run fallback
alone does not demonstrate that experience. No real bank or account sign-in is
part of this checklist.
