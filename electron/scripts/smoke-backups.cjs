async function finishRecovery(page) {
  await page.getByRole('button', {name:'Remind me later'}).click();
  await page.waitForURL(/\/settings\/book-protection\/$/);
  await Promise.all([page.waitForURL(/\/home\/$/), page.locator('a[href$="/home/"]').first().click()]);
}
'use strict';
// DESKTOP-BACKUPS: back up and restore through the real Backups page, then prove integrity.
// Isolated HOME/XDG and data folder; a seeded scratch book (harness screen scenario + receipts) unlocks
// through the real lock screen. Native dialogs are answered by stubbing Electron's dialog module in
// the main process (the page never sees a path). Run under xvfb-run: node scripts/smoke-backups.cjs
const { _electron: electron } = require('playwright');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const root = path.resolve(__dirname, '..');
const repo = path.resolve(root, '..');
const PASSWORD = 'backup-smoke-lock-2026';

module.exports = async () => {
  assert.ok(process.env.DISPLAY, 'run under xvfb-run');
  const base = process.env.FB_BACKUPS_SMOKE_DIR || fs.mkdtempSync(path.join(process.env.TMPDIR, 'fb-backups-smoke-'));
  fs.rmSync(base, { recursive: true, force: true });
  fs.mkdirSync(base, { recursive: true, mode: 0o700 });
  const shots = process.env.FB_SMOKE_SHOTS || path.join(root, 'staging-run');
  fs.mkdirSync(shots, { recursive: true });
  const home = path.join(base, 'home');
  const data = path.join(base, 'data');
  const books = path.join(data, 'books');
  const original = path.join(books, 'default.fbbook');
  const pw = path.join(base, 'lock-password');
  fs.mkdirSync(home, { recursive: true, mode: 0o700 });
  fs.writeFileSync(pw, PASSWORD + '\n', { mode: 0o600 });
  let checks = 0;
  const pass = text => console.log(`PASS BACKUPS ${String(++checks).padStart(2, '0')}: ${text}`);
  const shot = async (page, name) => { const f = path.join(shots, `${name}.png`); await page.screenshot({ path: f }); console.log(`      screenshot ${f}`); };
  const sha = f => crypto.createHash('sha256').update(fs.readFileSync(f)).digest('hex');
  const live = () => ['', '-wal', '-shm'].map(s => original + s).filter(f => fs.existsSync(f)).map(f => `${path.basename(f)}:${sha(f)}`).join(' ');
  const tree = dir => fs.readdirSync(dir, { recursive: true }).sort().join('\n');
  const build = spawnSync('cargo', ['build', '-q', '--release', '-p', 'fb-app', '--example', 'backup_proof'], { cwd: repo, stdio: 'inherit' });
  assert.equal(build.status, 0, 'build backup_proof');
  const proof = (...args) => {
    const r = spawnSync(path.join(repo, 'target/release/examples/backup_proof'), args, { cwd: repo, encoding: 'utf8' });
    process.stdout.write(r.stdout.split('\n').map(l => (l ? `      ${l}` : l)).join('\n'));
    if (r.stderr) process.stderr.write(r.stderr);
    return r.status;
  };
  assert.equal(proof('seed', data, pw, path.join(repo, 'crates/fb-screendiff/scenarios/screens-d7b.json')), 0);
  pass('seeded an isolated scratch book (invoices, bills, payments, deposits, reconciliations, receipts)');

  const env = { ...process.env, HOME: home, XDG_CONFIG_HOME: path.join(home, '.config'), XDG_DATA_HOME: path.join(home, '.local/share'),
    XDG_CACHE_HOME: path.join(home, '.cache'), XDG_STATE_HOME: path.join(home, '.local/state'), FB_DESKTOP_USER_DATA: data, ELECTRON_ENABLE_LOGGING: '0' };
  const app = await electron.launch({ args: [root, '--password-store=basic'], cwd: root, env, timeout: 60000 });
  let restoredBook;
  try {
    const page = await app.firstWindow();
    await page.setViewportSize({ width: 1440, height: 1000 });
    page.setDefaultTimeout(180000);
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    const stub = (method, result) => app.evaluate(({ dialog }, [m, r]) => { dialog[m] = async () => r; }, [method, result]);
    const notices = async () => (await page.locator('main p[role=status]').allInnerTexts()).join('\n');

    await page.fill('#id_password', PASSWORD);
    await Promise.all([page.waitForURL(/\/settings\/recovery-key\/$/), page.click('button:has-text("Unlock")')]);
    await finishRecovery(page);
    const company = /\/c\/([0-9a-f-]{36})\//.exec(page.url())[1];
    await page.goto(`fb://app/c/${company}/trial-balance/`);
    const originalTb = await page.locator('main').innerText();
    await page.goto(`fb://app/c/${company}/home/`);
    pass('unlocked the scratch book through the real lock screen');

    await Promise.all([page.waitForEvent('load'), page.click('nav a[href$="/backups/"]')]);
    assert.equal((await page.locator('main h1').innerText()).trim(), 'Backups');
    assert.match(await page.locator('.backup-status').innerText(), /None yet[\s\S]*Not verified yet[\s\S]*0 files/);
    assert.match(await page.locator('.backup-folder').innerText(), new RegExp(`${path.join(data, 'backups').replace(/[/.]/g, '\\$&')}.*created on first backup`));
    assert.doesNotMatch(await page.content(), /Not available yet|not in the desktop app yet/);
    await shot(page, 'backups-01-empty');
    pass('the nav link opens the real Backups page (not "Not available yet")');

    await Promise.all([page.waitForURL(/\/backups\/\?created=/), page.click('button:has-text("Create backup now")')]);
    const created = new URL(page.url()).searchParams.get('created');
    assert.match(created, /^freedombooks-default-\d{8}T\d{6}Z\.fbbackup$/);
    const said = await notices();
    assert.match(said, /Backup created: freedombooks-default-.*bytes\)\./);
    assert.match(said, /Verified: the backup opens/);
    const row = page.locator('table.backup-files tr.backup-new');
    assert.match(await row.innerText(), /New[\s\S]*Own password[\s\S]*Verified \d{4}-\d\d-\d\d/);
    assert.match(await page.locator('.backup-status').innerText(), /1 file/);
    const backup = path.join(data, 'backups', created);
    assert.ok(fs.statSync(backup).size > 100000 && fs.existsSync(backup + '.verified.json'));
    assert.equal(fs.statSync(path.join(data, 'backups')).mode & 0o777, 0o700);
    assert.equal(fs.statSync(backup).mode & 0o777, 0o600);
    await shot(page, 'backups-02-created');
    pass(`Create backup now wrote, verified and listed ${created} (0600 in a 0700 folder)`);

    const before = live();
    const booksBefore = tree(books);
    await page.fill('#id_new_book', 'Proof restore');
    await shot(page, 'backups-03-restore-form');
    await stub('showOpenDialog', { canceled: false, filePaths: [backup] });
    await Promise.all([page.waitForEvent('load'), page.click('button:has-text("Restore a backup as a new book")')]);
    assert.match(await notices(), /Restored “Proof restore” from freedombooks-default-.* into a new book\. Your open book was not changed\./);
    restoredBook = path.join(books, 'restored-proof-restore', 'restored-proof-restore.fbbook');
    assert.ok(fs.existsSync(restoredBook) && fs.statSync(path.join(books, 'restored-proof-restore', 'receipts')).isDirectory());
    assert.equal(live(), before, 'the open book files are byte-identical after the restore');
    assert.equal(await page.locator('form.backup-restored button:has-text("Open restored book")').count(), 1);
    await shot(page, 'backups-04-restored');
    pass('Restore a backup as a new book… made a separate book; the open book file hash is unchanged');

    const refuse = async (name, file, pattern, what) => {
      const snapshot = tree(books);
      await page.fill('#id_new_book', name);
      if (file) await stub('showOpenDialog', { canceled: false, filePaths: [file] });
      await Promise.all([page.waitForEvent('load'), page.click('button:has-text("Restore a backup as a new book")')]);
      const text = await page.locator('main p.errorlist').allInnerTexts();
      assert.match(text.join('\n'), pattern, `${what}: ${text.join(' | ')}`);
      assert.equal(tree(books), snapshot, `${what}: nothing created`);
      pass(what);
    };
    await refuse('Proof restore', null, /Not restored: A book named “Proof restore” already exists here/, 'restoring onto an existing book is refused');
    await refuse('Default', null, /Not restored: “Default” is reserved/, 'restoring onto the main book name is refused');
    const damaged = path.join(base, 'damaged.fbbackup');
    const bytes = fs.readFileSync(backup);
    bytes[Math.floor(bytes.length / 2)] ^= 0x55;
    fs.writeFileSync(damaged, bytes);
    assert.equal(await page.locator('form.backup-restored').count(), 0, 'a refusal clears the earlier "restored … is ready" panel');
    // The next restore: the picker starts in the backup folder, the last refusal is hidden while it runs,
    // and the busy line says other pages wait, with the time so far.
    assert.equal(await page.locator('main > p.errorlist:visible').count(), 1);
    await app.evaluate(({ dialog }, file) => {
      dialog.showOpenDialog = async (win, options) => {
        globalThis.fbSmokePicker = options;
        await new Promise(r => setTimeout(r, 2500));
        return { canceled: false, filePaths: [file] };
      };
    }, damaged);
    await page.fill('#id_new_book', 'Damaged copy');
    const refused = page.waitForEvent('load');
    await page.click('button:has-text("Restore a backup as a new book")');
    await page.locator('p.backup-busy').waitFor();
    assert.equal(await page.locator('main > p.errorlist:visible').count(), 0, 'the earlier refusal is hidden while the next restore runs');
    assert.match(await page.locator('p.backup-busy').innerText(), /^Restoring… .* Other pages open when it finishes\. Elapsed 0:0\d\.$/);
    await shot(page, 'backups-05a-restore-busy');
    assert.equal(await app.evaluate(() => globalThis.fbSmokePicker.defaultPath), path.join(data, 'backups'), 'the restore picker starts in the backup folder');
    await refused;
    assert.match((await page.locator('main p.errorlist').allInnerTexts()).join('\n'), /Not restored: This backup file is damaged or incomplete\. Nothing was restored\./);
    assert.doesNotMatch((await page.locator('main p.errorlist').allInnerTexts()).join('\n'), /reserved/);
    pass('a damaged backup is refused in plain words before any book is created; the picker started in the backup folder and the old refusal was hidden while it ran');
    assert.match(await page.locator('main p.errorlist small.message-detail').innerText(), /^Details: /);
    assert.equal(fs.existsSync(path.join(books, 'restored-damaged-copy')), false);
    await shot(page, 'backups-05-refused');
    assert.equal(live(), before);
    assert.equal(fs.readdirSync(path.join(data, 'work')).length, 0, 'no work files left behind');

    await stub('showOpenDialog', { canceled: false, filePaths: [path.join(books, 'elsewhere')] });
    await Promise.all([page.waitForEvent('load'), page.click('button:has-text("Choose folder…")')]);
    assert.match((await page.locator('main p.errorlist').allInnerTexts()).join('\n'), /Folder not changed: backups cannot go inside the books folder/);
    await stub('showOpenDialog', { canceled: false, filePaths: [base] });
    await Promise.all([page.waitForEvent('load'), page.click('button:has-text("Choose folder…")')]);
    assert.match((await page.locator('main p.errorlist').allInnerTexts()).join('\n'), /Folder not changed: this folder contains the books folder, and backups cannot contain the books folder\. Choose a folder outside the books folder/);
    await stub('showOpenDialog', { canceled: false, filePaths: [data] });
    await Promise.all([page.waitForEvent('load'), page.click('button:has-text("Choose folder…")')]);
    assert.match((await page.locator('main p.errorlist').allInnerTexts()).join('\n'), /cannot contain the books folder/);
    pass('Choose folder… refuses a folder inside the books folder, and one that contains it (a parent folder, the data folder) for that reason');

    // A damaged and a foreign file in the backup folder show as Unreadable (Django's row), not "Yes".
    const folderFiles = path.join(data, 'backups');
    const cut = path.join(folderFiles, 'freedombooks-default-20200101T000000Z.fbbackup');
    fs.writeFileSync(cut, fs.readFileSync(backup).subarray(0, 4096), { mode: 0o600 });
    const foreign = path.join(folderFiles, 'freedombooks-default-20200102T000000Z.fbbackup');
    fs.writeFileSync(foreign, 'not a backup at all', { mode: 0o600 });
    await page.reload();
    const encryptedColumn = async name => (await page.locator(`table.backup-files tr:has(code:text-is("${name}")) td[data-label=Protection]`).innerText()).trim();
    assert.equal(await encryptedColumn(path.basename(cut)), 'Unreadable');
    assert.equal(await encryptedColumn(path.basename(foreign)), 'Unreadable');
    assert.equal(await encryptedColumn(created), 'Own password');
    await shot(page, 'backups-08-unreadable');
    fs.rmSync(cut);
    fs.rmSync(foreign);
    await page.reload();
    pass('a truncated and a foreign file in the backup folder are marked Unreadable');

    const copy = path.join(base, 'copy-of-backup.fbbackup');
    await stub('showSaveDialog', { canceled: false, filePath: copy });
    await Promise.all([page.waitForEvent('load'), page.click(`table.backup-files a:has-text("Download")`)]);
    assert.equal(sha(copy), sha(backup));
    assert.equal(fs.statSync(copy).mode & 0o777, 0o600);
    assert.match(await notices(), /Saved a copy of freedombooks-default-/);
    pass('Download saves a byte-identical copy through the native Save dialog');

    // Crash residue: a dead process's partial copy and half-written receipts free the name again;
    // a live process's partial keeps it refused (a restore may still be running).
    const host = crypto.createHash('sha256').update(fs.readFileSync('/etc/hostname', 'utf8').trim()).digest('hex').slice(0, 16);
    const crashed = path.join(books, 'restored-after-crash');
    fs.mkdirSync(path.join(crashed, 'receipts'), { recursive: true, mode: 0o700 });
    fs.writeFileSync(path.join(crashed, `.fb-backup.${host}.4294967295.Kp3rash9.partial`), 'half a book');
    fs.writeFileSync(path.join(crashed, 'receipts', 'half.bin'), 'r');
    const running = path.join(books, 'restored-still-running');
    fs.mkdirSync(running, { mode: 0o700 });
    const livePartial = path.join(running, `.fb-backup.${host}.${process.pid}.Kp3live9.partial`);
    fs.writeFileSync(livePartial, 'in progress');
    await refuse('Still running', null, /Not restored: a restore into “Still running” is still running or did not finish\./, 'a live process\'s restore residue keeps the name refused');
    assert.ok(fs.existsSync(livePartial));
    await page.fill('#id_new_book', 'After crash');
    await stub('showOpenDialog', { canceled: false, filePaths: [backup] });
    await Promise.all([page.waitForEvent('load'), page.click('button:has-text("Restore a backup as a new book")')]);
    assert.match(await notices(), /Restored “After crash” from freedombooks-default-.* into a new book\./);
    assert.deepEqual(fs.readdirSync(crashed).sort(), ['receipts', 'restored-after-crash.fbbook', 'restored-after-crash.fbbook.keys.json']);
    assert.ok(!fs.existsSync(path.join(crashed, 'receipts', 'half.bin')), 'the half-written receipts went with the residue');
    fs.rmSync(running, { recursive: true });
    restoredBook = path.join(crashed, 'restored-after-crash.fbbook');
    pass('a dead process\'s crash residue is cleared and the name restores normally; the earlier refusals cleared the old ready panel');

    await page.fill('#id_new_book', '');
    await Promise.all([page.waitForURL(/\/home\/$/), page.click('button:has-text("Open restored book")')]);
    assert.match(await page.locator('.preview').first().innerText(), /Restored book “After crash” is open\. Lock \(File › Lock\) to return to your main book\./);
    const restoredCompany = /\/c\/([0-9a-f-]{36})\//.exec(page.url())[1];
    assert.equal(restoredCompany, company, 'the restored book holds the same companies');
    await shot(page, 'backups-06-restored-book-home');
    await page.goto(`fb://app/c/${restoredCompany}/trial-balance/`);
    assert.equal(await page.locator('main').innerText(), originalTb);
    await shot(page, 'backups-07-restored-trial-balance');
    pass('Open restored book shows the restored books (banner, Home, trial balance identical to the original)');

    await page.goto(`fb://app/c/${restoredCompany}/backups/`);
    await refuse('After crash', null, /already exists here/, 'from the restored book, restoring onto its own (open) book is refused');
    assert.deepEqual(errors, []);
    pass('no page errors under the production CSP');
  } finally {
    await app.close();
  }
  assert.ok(!fs.existsSync(original + '-wal') && !fs.existsSync(restoredBook + '-wal'), 'books closed cleanly');
  const proofDir = path.join(base, 'proof');
  fs.mkdirSync(proofDir, { recursive: true });
  assert.equal(proof('report', original, pw, data, path.join(proofDir, 'original.json')), 0);
  assert.equal(proof('report', restoredBook, pw, data, path.join(proofDir, 'restored.json')), 0);
  assert.equal(proof('compare', path.join(proofDir, 'original.json'), path.join(proofDir, 'restored.json')), 0, 'integrity comparison');
  pass('integrity: book check, reports, every table, attachments and audit trail match');
  assert.equal(proof('leakscan', data, pw, original), 0, 'leak scan');
  pass('no password or key in clear under the data folder; no book content in clear in backup files');
  await responsive(base, shots, env, proof, pass, shot);
  console.log(`SMOKE BACKUPS OK: ${checks} checks`);
};
// BACKUPS-POLISH: a large synthetic book (FB_BACKUPS_LARGE_MB of incompressible receipts, default 256) is
// backed up through the page while the main process is pinged; every ping must be answered promptly, the
// page shows the busy state with its buttons disabled, and a second command is refused until it is done.
async function responsive(base, shots, env0, proof, pass, shot) {
  const mb = process.env.FB_BACKUPS_LARGE_MB || '256';
  const data = path.join(base, 'large');
  const pw = path.join(base, 'lock-password');
  assert.equal(proof('seed-large', data, pw, path.join(repo, 'crates/fb-screendiff/scenarios/screens-d7b.json'), mb), 0);
  const env = { ...env0, FB_DESKTOP_USER_DATA: data };
  const app = await electron.launch({ args: [root, '--password-store=basic'], cwd: root, env, timeout: 60000 });
  try {
    const page = await app.firstWindow();
    await page.setViewportSize({ width: 1440, height: 1000 });
    page.setDefaultTimeout(600000);
    await page.fill('#id_password', PASSWORD);
    await Promise.all([page.waitForURL(/\/settings\/recovery-key\/$/), page.click('button:has-text("Unlock")')]);
    await finishRecovery(page);
    const company = /\/c\/([0-9a-f-]{36})\//.exec(page.url())[1];
    await page.goto(`fb://app/c/${company}/backups/`);
    const alerts = [];
    page.on('dialog', d => { alerts.push(d.message()); d.accept().catch(() => {}); });
    const started = Date.now();
    const navigated = page.waitForURL(/\/backups\/\?created=/, { timeout: 600000 });
    await page.click('button:has-text("Create backup now")');
    await page.locator('p.backup-busy').waitFor();
    assert.match(await page.locator('p.backup-busy').innerText(), /^Backing up… This can take a while for a large book\. Other pages open when it finishes\. Elapsed \d+:\d\d\.$/);
    assert.equal(await page.locator('main button:not([disabled])').count(), 0, 'every button is disabled while it runs');
    assert.equal(await page.locator('main').getAttribute('aria-busy'), 'true');
    await shot(page, 'backups-09-busy');
    // A second command while it runs (the disabled button bypassed): the main process refuses it.
    await page.evaluate(() => document.querySelector('input[name=action][value=folder]').form.requestSubmit());
    const pings = [];
    let refusedSecond = false;
    while (!(await settled(navigated))) {
      const t = process.hrtime.bigint();
      await app.evaluate(() => process.pid);
      pings.push(Number(process.hrtime.bigint() - t) / 1e6);
      if (alerts.some(a => /A backup or restore is still running/.test(a))) refusedSecond = true;
      await new Promise(r => setTimeout(r, 50));
    }
    const seconds = (Date.now() - started) / 1000;
    const worst = Math.max(...pings);
    console.log(`      backup of a ${mb} MB synthetic book took ${seconds.toFixed(1)} s; main process answered ${pings.length} pings meanwhile, slowest ${worst.toFixed(0)} ms`);
    assert.ok(seconds >= 2, `the large backup must run long enough to prove anything (${seconds}s)`);
    assert.ok(pings.length >= 10, `the main process answered only ${pings.length} pings`);
    assert.ok(worst < 1000, `a ping waited ${worst} ms: the main process was blocked`);
    assert.ok(refusedSecond, `the second command was refused while busy (alerts: ${JSON.stringify(alerts)})`);
    assert.match(await page.locator('main').innerText(), /Backup created: freedombooks-default-.*bytes\)\.[\s\S]*Verified: the backup opens/);
    assert.equal(await page.locator('p.backup-busy').count(), 0);
    assert.ok(await page.locator('button:has-text("Create backup now")').isEnabled());
    await shot(page, 'backups-10-large-created');
    pass(`the window stays responsive during a ${mb} MB backup (busy state, buttons disabled, second command refused, ${pings.length} pings, slowest ${worst.toFixed(0)} ms)`);
  } finally {
    await app.close();
  }
  fs.rmSync(data, { recursive: true, force: true });
}

async function settled(promise) {
  return Promise.race([promise.then(() => true, () => true), new Promise(r => setTimeout(() => r(false), 0))]);
}

if (require.main === module) module.exports().catch(error => { console.error(`SMOKE BACKUPS FAILED: ${error.stack || error}`); process.exitCode = 1; });
