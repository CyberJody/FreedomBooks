'use strict';
// Flatpak-style six checks, adapted to the hardened packaged shell: DOM forms
// and the real menu accelerator, with no test bridge or development overrides.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const {spawn, spawnSync} = require('node:child_process');
const {setTimeout: delay} = require('node:timers/promises');
const {chromium} = require(process.env.FB_SMOKE_PLAYWRIGHT || '../electron/node_modules/playwright');
const {signIn} = require('../electron/scripts/smoke-identity.cjs');
const binary = process.argv[2];
const shots = process.argv[3];
const label = process.argv[4] || 'INSTALLED';
const USER_PASSWORD = 'Synthetic installed owner42';
const settings = path.join(process.env.XDG_CONFIG_HOME, 'app.freedombooks.FreedomBooks');
const data = path.join(process.env.XDG_DATA_HOME, 'FreedomBooks');
let checks = 0, child, browser, port;
const pass = message => console.log(`PASS ${++checks}: ${message}`);
async function start() {
  const log = fs.openSync(path.join(shots, 'launch.log'), 'a');
  child = spawn(binary, ['--remote-debugging-address=127.0.0.1', `--remote-debugging-port=${port}`, '--disable-gpu', '--password-store=basic'],
    {stdio: ['ignore', log, log], detached: true});
  fs.closeSync(log);
  let launchError;
  child.once('error', error => {launchError = error;});
  for (let retry = 0; retry < 120; retry++) {
    if (launchError) throw launchError;
    if (child.exitCode !== null || child.signalCode) throw new Error(`App exited ${child.exitCode}/${child.signalCode}; see ${shots}/launch.log`);
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, {timeout: 1000}); }
    catch { await delay(500); continue; }
    const context = browser.contexts()[0];
    const page = context.pages().find(page => page.url().startsWith('fb://app/')) || await context.waitForEvent('page', {timeout: 30000});
    // Initial encrypted schema creation fsyncs many migrations. Allow a busy CI
    // disk to finish without changing any product behavior or assertions.
    page.setDefaultTimeout(300000);
    await page.waitForSelector('main h1');
    return page;
  }
  throw new Error(`No debug endpoint; see ${shots}/launch.log`);
}
async function stop() {
  if (browser) { await browser.close(); browser = null; }
  if (child && child.exitCode === null && !child.signalCode) {
    try { process.kill(-child.pid, 'SIGTERM'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
    for (let retry = 0; retry < 100 && child.exitCode === null && !child.signalCode; retry++) await delay(100);
    if (child.exitCode === null && !child.signalCode) process.kill(-child.pid, 'SIGKILL');
  }
  child = null;
}
async function heading(page) { return (await page.locator('main h1').first().textContent()).trim(); }
async function shot(page, name) { await page.screenshot({path: path.join(shots, name + '.png')}); }
(async () => {
  assert.ok(process.env.DISPLAY, 'Run under xvfb-run');
  assert.ok(binary && shots && process.env.XDG_CONFIG_HOME && process.env.XDG_DATA_HOME, 'Provide binary, screenshots, isolated XDG config/data homes');
  assert.ok(!fs.existsSync(data) && !fs.existsSync(settings), 'Refuse to touch an existing app profile or book folder');
  fs.mkdirSync(shots, {recursive: true});
  const probe = net.createServer();
  await new Promise((resolve, reject) => {probe.once('error', reject); probe.listen(0, '127.0.0.1', resolve);});
  port = probe.address().port;
  await new Promise(resolve => probe.close(resolve));
  try {
    const page = await start();
    assert.equal(await heading(page), 'Set up your books');
    await shot(page, '01-first-run');
    pass('installed app opens the first-run window');
    assert.deepEqual(await page.evaluate(() => ({require: typeof window.require, process: typeof window.process, fb: typeof window.fb})),
      {require: 'undefined', process: 'undefined', fb: 'undefined'});
    assert.equal(await page.locator('form').count() > 0, true);
    const cdp = await browser.newBrowserCDPSession();
    const {processInfo} = await cdp.send('SystemInfo.getProcessInfo');
    const renderers = processInfo.filter(info => info.type === 'renderer');
    assert.ok(renderers.length > 0, 'renderer process exists');
    for (const renderer of renderers) {
      const status = fs.readFileSync(`/proc/${renderer.id}/status`, 'utf8');
      assert.match(status, /^Seccomp:\s+2$/m, 'renderer seccomp filter is active');
      assert.match(status, /^NoNewPrivs:\s+1$/m, 'renderer cannot gain privileges');
      console.log(`SANDBOX VERIFIED renderer ${renderer.id}: Seccomp=2, NoNewPrivs=1`);
    }
    await cdp.detach();
    pass('renderer sandbox is active; no Node require/process or public command bridge; setup form is available');
    await page.fill('#id_owner_name', 'Dana Morgan');
    await page.fill('#id_company_name', 'Kettle Pond Supply (Demo)');
    // The default first run: "Protect these books with my own password" stays unchecked (built-in key).
    assert.equal(await page.isChecked('#id_protect'), false);
    assert.equal(await page.isDisabled('#id_lock_password'), true);
    assert.match(await page.locator('#built-in-warning').innerText(), /Anyone who gets a copy of this file can open it\./);
    try {
      await Promise.all([page.waitForURL(/\/desktop\/sign-in\/$/), page.click('button:has-text("Create books")')]);
      await signIn(page, USER_PASSWORD);
    } catch (error) {
      console.error('SETUP FAILURE:', page.url(), await page.locator('body').innerText());
      await shot(page, 'failure-setup');
      throw error;
    }
    assert.equal(await heading(page), 'Home');
    await shot(page, '02-home');
    pass('first run with the default built-in key creates synthetic books and renders Home');
    if (process.env.FB_SMOKE_REQUIRE_SIMPLEFIN === '1') {
      const companyBase = page.url().replace(/home\/$/, '');
      await page.goto(companyBase + 'settings/');
      await page.getByRole('link', {name: 'Bank feeds and rules', exact: true}).click();
      await page.waitForURL(/\/bank-feeds\/$/);
      assert.match(await heading(page), /Bank feeds/i);
      await shot(page, '02-bank-feeds');
      console.log('SIMPLEFIN VERIFIED: Bank feeds and rules is present and opens in the packaged app');
      await page.goto(companyBase + 'home/');
    }
    // "Host these books": the packaged remote-access helper is found at its fixed path, and
    // hosting is off by default (nothing listens; this smoke never turns it on).
    const homeUrl = page.url();
    await page.goto('fb://app/hosting/');
    assert.equal(await heading(page), 'Host these books');
    const hostingText = await page.locator('main').innerText();
    assert.ok(!/helper is missing/.test(hostingText), hostingText);
    assert.match(hostingText, /Hosting is off/);
    await shot(page, '02-hosting-off');
    console.log('HOSTING VERIFIED: the packaged fb-remote helper is found and hosting is off by default');
    await page.goto(homeUrl);
    const book = path.join(data, 'books/default.fbbook');
    assert.notEqual(fs.readFileSync(book).subarray(0, 16).toString(), 'SQLite format 3\0');
    assert.equal(fs.statSync(book).mode & 0o077, 0);
    const record = path.join(data, 'books/default.fbbook.keys.json');
    assert.equal(fs.statSync(record).mode & 0o077, 0);
    const keys = JSON.parse(fs.readFileSync(record, 'utf8'));
    assert.equal(keys.keys.mode, 'built-in');
    assert.equal(keys.keys.password, null);
    for (const gone of ['book-key.lock.json', 'book-key.sealed', 'books/default.fbbook.os-key']) assert.ok(!fs.existsSync(path.join(data, gone)), gone);
    assert.ok(!fs.existsSync(path.join(settings, 'books/default.fbbook')));
    pass('book is a private SQLCipher file with a built-in-key record and no password wrap or sealed key');
    // CDP dispatches renderer input only; use real X11 input for the native
    // Electron menu accelerator. No main-process inspector/test bridge needed.
    const window = spawnSync('xdotool', ['search', '--onlyvisible', '--class', 'app.freedombooks.FreedomBooks'], {encoding: 'utf8'});
    assert.equal(window.status, 0, window.stderr);
    const windowId = window.stdout.trim().split('\n')[0];
    for (const args of [['windowfocus', '--sync', windowId], ['key', '--clearmodifiers', 'ctrl+l']]) {
      const result = spawnSync('xdotool', args, {encoding: 'utf8'});
      assert.equal(result.status, 0, result.stderr);
    }
    await delay(2000);
    await signIn(page, USER_PASSWORD);
    assert.equal(await heading(page), 'Home');
    await page.goto('fb://app/settings/book-protection/');
    assert.equal(await heading(page), 'Book protection');
    assert.match(await page.locator('main').innerText(), /Anyone who gets a copy of this file can open it\.[\s\S]*File › Lock, idle lock and switching users still end the individual sign-in session\./);
    await shot(page, '03-book-protection');
    pass('File > Lock signs out of the built-in-key book; Book protection explains the file key');
    await stop();
    const reopened = await start();
    await signIn(reopened, USER_PASSWORD);
    assert.equal(await heading(reopened), 'Home');
    assert.match(reopened.url(), /\/home\/$/);
    await shot(reopened, '04-home-after-restart');
    pass('built-in-key book persists across restart and opens straight to Home');
    console.log(`${label} SMOKE OK: ${checks} checks; screenshots: ${shots}`);
  } finally { await stop(); }
})().catch(error => {console.error(error.stack || error); process.exitCode = 1;});
