// Remote access, real isolated Electron UI as a thin client of a synthetic host (crates/fb-host
// example dev_host) on 127.0.0.1. Synthetic book, names and passwords only; an isolated Xvfb
// display and profile. The keyring is a synthetic test wrapper (as smoke-keys does).
//
//   FB_REMOTE_HELPER=<target>/debug/fb-remote FB_DEV_HOST=<target>/debug/examples/dev_host node scripts/smoke-remote.cjs
'use strict';
const {_electron: electron} = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const readline = require('node:readline');
const {spawn} = require('node:child_process');
const root = path.resolve(__dirname, '..');
const KEEPER = 'Synthetic bookkeeper';
const PASSWORD = 'Synthetic individual password';
const IDLE_SECONDS = 5;
const SECOND_HOST = 'Synthetic branch host';
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

async function main() {
  assert.ok(process.env.DISPLAY && ![':0', ':20', ':0.0', ':20.0'].includes(process.env.DISPLAY), 'isolated Xvfb display required');
  const helper = process.env.FB_REMOTE_HELPER, hostExe = process.env.FB_DEV_HOST;
  assert.ok(helper && fs.existsSync(helper), 'FB_REMOTE_HELPER must name a built fb-remote');
  assert.ok(hostExe && fs.existsSync(hostExe), 'FB_DEV_HOST must name the built dev_host example');
  const base = fs.mkdtempSync(path.join(process.env.TMPDIR, 'fb-remote-smoke-'));
  const home = path.join(base, 'home'), data = path.join(base, 'data');
  fs.mkdirSync(home);
  const shots = process.env.FB_SMOKE_SHOTS || path.join(base, 'screenshots');
  fs.mkdirSync(shots, {recursive: true});
  let checks = 0;
  const pass = s => console.log(`PASS REMOTE ${++checks}: ${s}`);

  const startHost = (name, ...options) => {
    const dir = path.join(base, name);
    fs.mkdirSync(dir);
    const child = spawn(hostExe, [dir, helper, ...options], {stdio: ['pipe', 'pipe', 'inherit']});
    const exited = new Promise(resolve => child.once('close', resolve));
    const lines = readline.createInterface({input: child.stdout})[Symbol.asyncIterator]();
    const next = async () => JSON.parse((await lines.next()).value);
    return {pid: child.pid, next, ask: async command => { child.stdin.write(command + '\n'); return next(); },
      async close() {
        child.stdin.end();
        const kill = setTimeout(() => child.kill('SIGKILL'), 5000);
        await exited;
        clearTimeout(kill);
        if (!process.env.FB_SMOKE_KEEP) fs.rmSync(dir, {recursive: true, force: true});
      },
    };
  };
  const entry = path.join(home, 'test-keyring.cjs');
  // This wrapper runs only in the test, with an intentionally synthetic keyring key.
  fs.writeFileSync(entry, `const {safeStorage}=require('electron');const c=require('node:crypto');\nObject.assign(safeStorage,{isEncryptionAvailable:()=>true,getSelectedStorageBackend:()=> 'gnome_libsecret',encryptString:s=>{const n=c.randomBytes(12),x=c.createCipheriv('aes-256-gcm',Buffer.alloc(32,23),n);const b=Buffer.concat([x.update(s,'utf8'),x.final()]);return Buffer.concat([n,x.getAuthTag(),b]);},decryptString:b=>{const x=c.createDecipheriv('aes-256-gcm',Buffer.alloc(32,23),b.subarray(0,12));x.setAuthTag(b.subarray(12,28));return Buffer.concat([x.update(b.subarray(28)),x.final()]).toString('utf8');}});require(${JSON.stringify(path.join(root, 'main/entry.cjs'))});\n`, {mode: 0o600});
  const env = {...process.env, HOME: home, XDG_CONFIG_HOME: path.join(home, 'config'), XDG_DATA_HOME: path.join(home, 'share'), XDG_CACHE_HOME: path.join(home, 'cache'), FB_DESKTOP_USER_DATA: data, FB_REMOTE_HELPER: helper, ELECTRON_ENABLE_LOGGING: '0'};
  let app, page;
  const launch = async () => {
    app = await electron.launch({args: [entry, '--password-store=basic'], cwd: root, env, timeout: 180000});
    await app.evaluate(({dialog}) => { dialog.showErrorBox = (_t, m) => { throw new Error(m); }; });
    page = await app.firstWindow();
    page.setDefaultTimeout(45000);
    await page.setViewportSize({width: 1440, height: 1000});
    await page.waitForSelector('h1');
  };
  const shot = n => page.screenshot({path: path.join(shots, n + '.png')});
  const menu = id => app.evaluate(({Menu}, id) => Menu.getApplicationMenu().getMenuItemById(id).click(), id);
  // Wait for the window to settle on a path; navigations replaced by the next one (the lock's
  // "Locked" page, then the sign-in screen) abort, which waitForURL reports as an error.
  const at = async (re, timeout = 30000) => {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      if (re.test(new URL(page.url()).pathname)) { await page.waitForLoadState('load').catch(() => {}); if (re.test(new URL(page.url()).pathname)) return; }
      await delay(100);
    }
    throw new Error(`timed out waiting for ${re}; at ${page.url()}`);
  };
  const title = () => app.evaluate(({BrowserWindow}) => BrowserWindow.getAllWindows()[0].getTitle());
  const marker = () => page.locator('meta[name=fb-remote-page]').getAttribute('content');
  const host = startHost('host', '--idle-seconds', String(IDLE_SECONDS));
  let branchHost;
  try {
    const started = await host.next();
    assert.match(started.addr, /^127\.0\.0\.1:\d+$/);
    await launch();
    // A computer with no books: the local first-run screen, with the connect choice in the File menu.
    assert.ok(await page.locator('#id_owner_name').count(), 'local first-run screen');
    await menu('remote-connect');
    await page.waitForURL(/\/remote\/connect\/$/);
    await shot('remote-01-connect');
    await page.fill('input[name=addr]', started.addr);
    await page.fill('input[name=code]', 'AAAA-AAAA-AAAA-AAAA');
    await page.fill('input[name=device_name]', 'Synthetic laptop');
    await page.getByRole('button', {name: 'Pair and connect', exact: true}).click();
    await page.waitForSelector('.fb-remote-errors');
    assert.match(await page.locator('.fb-remote-errors').innerText(), /did not work/);
    pass('a wrong pairing code is refused with one plain message');

    await page.fill('input[name=addr]', started.addr);
    await page.fill('input[name=code]', started.code.toLowerCase());
    await page.fill('input[name=device_name]', 'Synthetic laptop');
    await page.getByRole('button', {name: 'Pair and connect', exact: true}).click();
    await page.waitForURL(/\/desktop\/sign-in\/$/);
    assert.match(await page.locator('#fb-remote-bar').innerText(), /Connected to Synthetic office host/);
    assert.match(await title(), /connected to Synthetic office host/);
    assert.ok(fs.existsSync(path.join(data, 'remote-host.sealed')), 'the pairing is kept sealed');
    assert.ok(!fs.readFileSync(path.join(data, 'remote-host.sealed'), 'latin1').includes('Synthetic office host'), 'sealed, not plain');
    await shot('remote-02-host-sign-in');
    pass('pairing with the host code connects and shows the host’s sign-in with a connected-host bar and title');

    await page.fill('#id_username', KEEPER);
    await page.fill('#id_password', 'not the password');
    await page.getByRole('button', {name: 'Sign in', exact: true}).click();
    await page.waitForSelector('.errorlist');
    await page.fill('#id_username', KEEPER);
    await page.fill('#id_password', PASSWORD);
    await page.getByRole('button', {name: 'Sign in', exact: true}).click();
    await page.waitForURL(/\/home\/$/);
    assert.match(await page.locator('.operator').innerText(), /Synthetic bookkeeper/);
    pass('application sign-in on the host is required; a wrong password is refused, the right one signs in');

    await page.goto(`fb://app/c/${started.company}/lists/customers/new/`);
    await page.fill('#id_display_name', 'Remote smoke customer');
    await page.getByRole('button', {name: /^Save/}).first().click();
    await page.waitForURL(url => !String(url).endsWith('/new/'));
    await page.goto(`fb://app/c/${started.company}/lists/customers/`);
    assert.match(await page.locator('main').innerText(), /Remote smoke customer/);
    await shot('remote-03-customer-saved');
    pass('a form saved in the client is saved in the host’s book');

    await page.goto('fb://app/settings/book-protection/');
    assert.match(await page.locator('main').innerText(), /Only on the host computer/);
    assert.equal(await page.locator('form[data-gate]').count(), 0, 'a page at a host address carries no pairing or disconnect form');
    assert.ok(await page.locator('a[href="/remote/disconnect/"]').count(), 'it links to the disconnect screen instead');
    pass('host-only screens are refused in the client, and their notice carries no connection forms');

    // The page marker must not take decisions away from the page's own scripts (review of c5a1e486):
    // the Bills list refuses "Pay selected" with nothing ticked, and a link that enters the page
    // after it loaded still reaches its page instead of the home page.
    await page.goto(`fb://app/c/${started.company}/bills/`);
    await page.waitForSelector('#pay-selected-button');
    const billsList = page.url();
    await page.locator('#pay-selected-button').click();
    await page.waitForTimeout(700);
    assert.equal(page.url(), billsList, 'a GET submit the page script cancelled is not sent');
    await page.evaluate(() => {
      const late = document.createElement('a');
      late.id = 'smoke-late-link'; late.textContent = 'Drafts'; late.setAttribute('href', '?state=DRAFT');
      document.querySelector('main').append(late);
    });
    await page.locator('#smoke-late-link').click();
    await page.waitForURL(url => new URL(String(url)).searchParams.get('state') === 'DRAFT');
    assert.equal(new URL(page.url()).pathname, new URL(billsList).pathname, 'it stays on the Bills list, filtered');
    assert.match(await page.locator('#fb-remote-bar').innerText(), /Connected to Synthetic office host/);
    pass('the page’s own scripts keep control of GET forms, and a link added after load carries the page marker');

    const signIn = async () => {
      await page.fill('#id_username', KEEPER);
      await page.fill('#id_password', PASSWORD);
      await page.getByRole('button', {name: 'Sign in', exact: true}).click();
      await page.waitForURL(/\/home\/$/);
    };
    // Lock with the host reachable: this person is signed out there.
    await page.goto(`fb://app/c/${started.company}/home/`);
    await menu('lock-books');
    await at(/^\/desktop\/sign-in\/$/);
    await page.goto(`fb://app/c/${started.company}/home/`);
    await at(/^\/desktop\/sign-in\/$/);
    pass('Lock signs this person out on the host');

    // Extra Lock events while the sign-in is open must leave the form and connection alone.
    await page.fill('#id_username', 'Unsubmitted synthetic name');
    await page.fill('#id_password', 'Unsubmitted synthetic password');
    await menu('lock-books');
    await app.evaluate(({powerMonitor}) => {
      powerMonitor.emit('lock-screen');
      powerMonitor.emit('suspend');
    });
    assert.equal(await page.locator('#id_username').inputValue(), 'Unsubmitted synthetic name');
    assert.equal(await page.locator('#id_password').inputValue(), 'Unsubmitted synthetic password');
    assert.match(await page.locator('#fb-remote-bar').innerText(), /Connected to Synthetic office host/);
    pass('further manual and simulated OS locks leave the sign-in form and connection intact');

    const token = page.locator('input[name=fb_command_token]');
    const lockedToken = await token.inputValue();
    const lockedMarker = await marker();
    await delay((IDLE_SECONDS + 2) * 1000);
    assert.equal(await token.inputValue(), lockedToken);
    assert.equal(await marker(), lockedMarker);
    assert.equal(await page.locator('#id_password').inputValue(), 'Unsubmitted synthetic password');
    await signIn();
    assert.equal(await marker(), lockedMarker, 'sign-in succeeded without replacing the session');
    pass('the same locked form signs in on the first try after the host idle limit');

    await delay((IDLE_SECONDS + 2) * 1000);
    await page.goto(`fb://app/c/${started.company}/home/`);
    await at(/^\/desktop\/sign-in\/$/);
    assert.notEqual(await marker(), lockedMarker);
    pass('a signed-in session still expires at the host idle limit');

    await signIn();
    assert.equal((await host.ask('end-sessions')).sessions, 0);
    await menu('lock-books');
    await at(/^\/desktop\/sign-in\/$/);
    await page.waitForSelector('#id_password');
    assert.match(await page.locator('#fb-remote-bar').innerText(), /Connected to Synthetic office host/);
    pass('Lock after the host has ended the session reconnects and shows its sign-in');

    // Lock while the host cannot answer (its book process frozen): cover the page and drop the
    // session, then reconnect automatically when this same host answers.
    await signIn();
    process.kill(host.pid, 'SIGSTOP');
    try {
      await menu('lock-books');
      await at(/^\/remote\/locked\/$/, 3000);
      assert.doesNotMatch(await page.locator('body').innerText(), /Remote smoke customer|Synthetic bookkeeper/);
      await shot('remote-05-locked');
      await page.getByRole('heading', {name: 'Locked. Reconnecting to Synthetic office host…'}).waitFor({timeout: 20000});
      await page.getByRole('link', {name: 'Try now'}).waitFor();
      await page.getByRole('link', {name: 'Disconnect', exact: true}).waitFor();
      await shot('remote-06-locked-unreachable');
    } finally {
      process.kill(host.pid, 'SIGCONT');
    }
    await at(/^\/desktop\/sign-in\/$/);
    await page.goto(`fb://app/c/${started.company}/home/`);
    await at(/^\/desktop\/sign-in\/$/);
    await signIn();
    pass('Lock with the host unreachable fails closed: no host page stays, and reconnecting needs a sign-in');

    assert.equal(fs.existsSync(path.join(data, 'books')) && fs.readdirSync(path.join(data, 'books')).some(n => n.endsWith('.fbbook')), false, 'the client computer has no book of its own');
    pass('the client never creates or opens a local book');

    await menu('lock-books');
    await at(/^\/desktop\/sign-in\/$/);
    const oldMarker = await marker();
    branchHost = startHost('branch-host', '--host-name', SECOND_HOST);
    const branch = await branchHost.next();
    assert.match(branch.addr, /^127\.0\.0\.1:\d+$/);
    assert.notEqual(branch.addr, started.addr);
    await menu('remote-connect');
    await at(/^\/remote\/connect\/$/);
    await page.fill('input[name=addr]', branch.addr);
    await page.fill('input[name=code]', branch.code);
    await page.fill('input[name=device_name]', 'Synthetic laptop');
    const sealed = path.join(data, 'remote-host.sealed');
    const oldPairing = fs.readFileSync(sealed);
    process.kill(branchHost.pid, 'SIGSTOP');
    try {
      await page.getByRole('button', {name: 'Pair and connect', exact: true}).click();
      await at(/^\/remote\/connecting\/$/);
      const deadline = Date.now() + 5000;
      while (fs.readFileSync(sealed).equals(oldPairing) && Date.now() < deadline) await delay(50);
      assert.ok(!fs.readFileSync(sealed).equals(oldPairing), 'Pair completed before the waiting Connect');
      await page.goto('fb://app/');
      await at(/^\/remote\/connecting\/$/);
      assert.equal(await page.locator('h1').innerText(), 'Connecting to a FreedomBooks host…');
      assert.equal(await page.locator('#id_password, #fb-remote-bar, meta[name=fb-remote-page]').count(), 0);
      assert.equal(await title(), 'Connecting — FreedomBooks');
      await shot('remote-07-changing-host');
    } finally {
      process.kill(branchHost.pid, 'SIGCONT');
    }
    await at(/^\/desktop\/sign-in\/$/);
    assert.notEqual(await marker(), oldMarker);
    assert.equal(await page.locator('#fb-remote-bar strong').innerText(), SECOND_HOST);
    assert.ok((await title()).endsWith(` — connected to ${SECOND_HOST}`));
    await shot('remote-08-second-host');
    await signIn();
    await page.goto(`fb://app/c/${branch.company}/reports/balance-sheet/`);
    await page.selectOption('[name=basis]', 'CASH');
    await Promise.all([
      page.waitForURL(/basis=CASH/),
      page.getByRole('button', {name: 'Run report', exact: true}).click(),
    ]);
    assert.match(await page.locator('.report-basis').innerText(), /Cash basis/);
    pass('the current host’s report filters still submit through the real GET form');
    await page.goto(`fb://app/c/${branch.company}/lists/customers/`);
    assert.doesNotMatch(await page.locator('main').innerText(), /Remote smoke customer/);
    await page.goto(`fb://app/c/${started.company}/home/?__fb_remote_page=${oldMarker}`);
    await at(/^\/$/);
    assert.notEqual(await marker(), oldMarker);
    assert.doesNotMatch(await page.locator('body').innerText(), /Synthetic office host|Remote smoke customer/);
    assert.equal(await page.locator('#fb-remote-bar strong').innerText(), SECOND_HOST);
    assert.ok((await title()).endsWith(` — connected to ${SECOND_HOST}`));
    pass('leaving Connect during a real host switch shows no host content until the new host is adopted, then refuses an old page query');

    // Revocation on the host: the next page says this computer is not accepted, and client mode ends.
    const devices = (await branchHost.ask('devices')).devices;
    assert.equal(devices.length, 1);
    assert.equal((await branchHost.ask('revoke ' + devices[0].id)).revoked, true);
    await page.goto(`fb://app/c/${branch.company}/home/`);
    await at(/^\/remote\/connect\/$/);
    assert.match(await page.locator('.fb-remote-errors').innerText(), /did not accept this computer/);
    await shot('remote-04-revoked');
    await page.getByRole('link', {name: 'Back to the books on this computer'}).click();
    await page.waitForSelector('#id_owner_name');
    pass('a revoked computer is refused and returns to its own (empty) books');
    console.log(`remote smoke: ${checks} checks passed; screenshots in ${shots}`);
    // Part 2: the desktop's own "Host these books" with a second real app (smoke-hosting.cjs).
    if (app) { await app.close().catch(() => {}); app = null; }
    const hosted = await require('./smoke-hosting.cjs').run({helper, shots, label: 'REMOTE HOSTING'});
    console.log(`remote smoke: ${checks} + ${hosted} hosting checks passed`);
  } finally {
    if (app) await app.close().catch(() => {});
    await Promise.all([host.close(), branchHost?.close()]);
  }
}
main().catch(e => { console.error(e); process.exit(1); });
