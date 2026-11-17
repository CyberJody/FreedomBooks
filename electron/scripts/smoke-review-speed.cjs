'use strict';
// Review transactions in the real app (review 2026-11-10): the real Electron shell, preload and fb-node
// module on an isolated synthetic book. Uses the AI smoke's fixture (a builtin-key book with a mapped bank
// feed and one downloaded debit; `cargo run -p fb-app --example ai_synthetic_fixture -- <dir> --builtin`) and
// its in-process OS-store simulator, so no desktop keyring, network or bank is involved.
// From the keyboard: find a category by typing part of its name, Enter to accept in place (no page load;
// the row shows its outcome), Undo in one confirmed step, I to ignore; letters typed in Payee stay text.
// After a reconnect (fixture `--reconnect`): the old connection's accepted row that the new copy is linked to
// offers no Undo and says why; an Undo pressed anyway through the real token path is refused and voids nothing.
// node scripts/smoke-review-speed.cjs <fixture dir> <fb-node.node>   (under a private Xvfb)
// Fixture: cargo run -p fb-app --example ai_synthetic_fixture -- <dir> --builtin --reconnect
const {_electron: electron} = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const fixture = JSON.parse(fs.readFileSync(path.join(process.argv[2], 'fixture.json')));
const nativePath = path.resolve(process.argv[3]);
const shots = process.env.FB_SMOKE_SHOTS || path.join(root, 'staging-run');
fs.mkdirSync(shots, {recursive: true});
const data = fs.mkdtempSync(path.join(process.env.TMPDIR || '/tmp', 'fb-review-speed-'));
assert.equal(fixture.builtin, true, 'generate the fixture with --builtin');
assert.ok(Number.isInteger(fixture.reconnect_old), 'generate the fixture with --reconnect');
fs.mkdirSync(path.join(data, 'books'), {recursive: true});
const book = path.join(data, 'books', 'review-fixture.fbbook');
fs.copyFileSync(fixture.book, book);
fs.copyFileSync(fixture.book + '.keys.json', book + '.keys.json');
const entry = path.join(data, 'entry.cjs');
// The same synthetic OS-store simulator as smoke-ai.cjs (a fixed in-memory AES key, never a real keyring).
fs.writeFileSync(entry, `const {safeStorage}=require('electron');const c=require('node:crypto');
Object.assign(safeStorage,{isEncryptionAvailable:()=>true,getSelectedStorageBackend:()=> 'gnome_libsecret',encryptString:s=>{const n=c.randomBytes(12),x=c.createCipheriv('aes-256-gcm',Buffer.alloc(32,19),n);const b=Buffer.concat([x.update(s,'utf8'),x.final()]);return Buffer.concat([n,x.getAuthTag(),b]);},decryptString:b=>{const x=c.createDecipheriv('aes-256-gcm',Buffer.alloc(32,19),b.subarray(0,12));x.setAuthTag(b.subarray(12,28));return Buffer.concat([x.update(b.subarray(28)),x.final()]).toString('utf8');}});require(${JSON.stringify(path.join(root, 'main/entry.cjs'))});`, {mode: 0o600});
const env = {...process.env, FB_DESKTOP_USER_DATA: data, FB_NATIVE_MODULE: nativePath, ELECTRON_ENABLE_LOGGING: '0'};
let checks = 0;
const pass = text => console.log(`PASS REVIEW ${++checks}: ${text}`);

(async () => {
  assert.ok(process.env.DISPLAY && ![':0', ':0.0', ':20', ':20.0'].includes(process.env.DISPLAY), 'isolated Xvfb display required');
  const app = await electron.launch({chromiumSandbox: true, args: [entry], cwd: root, env, timeout: 120000});
  const page = await app.firstWindow();
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  const shot = name => page.screenshot({path: path.join(shots, `review-speed-${name}.png`)});
  try {
    await page.setViewportSize({width: 1440, height: 900});
    page.setDefaultTimeout(120000);
    await app.evaluate(({app: a}, input) => {
      const req = process.getBuiltinModule('module').createRequire(input.nativePath);
      req(input.nativePath).openBook(input.book, input.key);
    }, {nativePath, book, key: fixture.key_hex});
    await page.goto(`fb://app/c/${fixture.company_id}/home/`);
    await page.waitForURL(/\/desktop\/sign-in\/$/);
    await page.fill('#id_username', fixture.username);
    await page.fill('#id_password', fixture.user_password);
    await page.getByRole('button', {name: 'Sign in', exact: true}).click();
    await page.waitForURL(url => url.pathname !== '/desktop/sign-in/');
    const posted = () => app.evaluate((_e, input) => JSON.parse(process.mainModule.require(input.nativePath).query(JSON.stringify({type: 'Register', company_id: input.company, account_id: input.bank}))), {nativePath, company: fixture.company_id, bank: fixture.bank});
    const before = await posted();
    await page.goto(`fb://app/c/${fixture.company_id}/bank-feeds/review/`);
    assert.equal(await page.locator('main h1').first().innerText(), 'Review transactions');
    await page.waitForSelector('tr[data-review-row] .combo-input');
    await page.evaluate(() => { window.__samePage = 'yes'; });
    const row = page.locator('tr[data-review-row]', {hasText: 'Synthetic office supplies'});
    const pk = await row.getAttribute('data-review-row');
    const current = () => page.locator(`#review-row-${pk}`);
    assert.equal(await row.locator('input[name^="number_"]').inputValue(), '', 'no invented check number');
    assert.equal(await page.isHidden('#review-bulk'), true, 'the selection bar waits for a selection');
    await shot('1-review');
    pass('signed in to the isolated fixture book; the review row and key legend render under the CSP');

    await row.focus();
    await page.keyboard.press('c');
    await page.keyboard.type('offi');
    await page.waitForSelector('.combo-list:not([hidden]) [role=option][aria-selected=true]');
    assert.match(await page.locator('.combo-list:not([hidden]) [role=option][aria-selected=true]').innerText(), /500 · Office/);
    await shot('2-category-search');
    await page.keyboard.press('Enter');
    assert.equal(await row.evaluate(el => el === document.activeElement), true, 'Enter in the search chose and returned to the row');
    assert.equal(await row.locator(`select[name="account_${pk}"]`).inputValue(), fixture.expense);
    pass('C then typing part of the name found the category; Enter chose it and returned to the row');

    await page.keyboard.press('Enter');
    await page.waitForSelector(`#review-row-${pk}[data-status=ACCEPTED]`);
    assert.match(await current().innerText(), /Accepted: CHK-/);
    assert.equal(await page.evaluate(() => window.__samePage), 'yes', 'no page load');
    assert.notDeepEqual(await posted(), before, 'the native accounting boundary recorded it');
    assert.match(await page.locator('#review-status').innerText(), /0 to review/);
    await shot('3-accepted-in-place');
    pass('Enter accepted in place: the row shows its outcome with Undo, the count updated, no page load');

    await current().focus();
    await page.keyboard.press('Enter');
    await page.waitForSelector('text=Confirm undo');
    assert.equal(await page.locator('button:has-text("Confirm undo")').evaluate(el => el === document.activeElement), true);
    await shot('4-undo-confirm');
    await page.keyboard.press('Enter');
    await page.waitForSelector(`#review-row-${pk}[data-status=NEW]`);
    assert.equal(await page.evaluate(() => window.__samePage), 'yes');
    pass('Enter on the accepted row offered Undo; confirming voided the check and returned the row to review');

    const payee = current().locator('input[name^="payee_"]');
    await payee.fill('');
    await payee.type('Ivy jk');
    assert.equal(await payee.inputValue(), 'Ivy jk', 'row keys never take letters typed in a text field');
    await current().focus();
    await page.keyboard.press('i');
    await page.waitForSelector(`#review-row-${pk}[data-status=IGNORED]`);
    assert.equal(await page.evaluate(() => window.__samePage), 'yes');
    await shot('5-ignored');
    pass('letters typed in Payee stayed text; I ignored the row in place');

    await page.goto(`fb://app/c/${fixture.company_id}/bank-feeds/review/?status=ACCEPTED`);
    const old = page.locator(`#review-row-${fixture.reconnect_old}`);
    await old.waitFor();
    assert.equal(await old.locator('button[name=undo_one]').count(), 0, 'no Undo on a row a re-download is linked to');
    assert.match(await old.innerText(), /Undo not available: the copy downloaded again \(Synthetic Savings 2026-10-30 Synthetic reconnect fee\) is linked to it\. Return that copy to review first\./);
    assert.match(await old.innerText(), /void or reverse JE \d+ from its page/);
    await shot('6-undo-not-available');
    const reversals = () => app.evaluate((_e, input) => JSON.parse(process.mainModule.require(input.nativePath).query(JSON.stringify({type: 'Register', company_id: input.company, account_id: input.bank}))), {nativePath, company: fixture.company_id, bank: fixture.savings});
    const register = await reversals();
    await page.evaluate(() => { window.__samePage = 'again'; });
    // A stale page's Undo, pressed anyway: the real preload sends it with the form's single-use token.
    await old.evaluate((row, pk) => {
      const button = document.createElement('button');
      button.type = 'submit'; button.name = 'undo_confirm_one'; button.value = String(pk); button.id = 'stale-undo';
      row.querySelector('.row-actions').append(button);
    }, fixture.reconnect_old);
    await page.click('#stale-undo');
    await page.waitForSelector('#review-status .results li.bad');
    assert.match(await page.locator('#review-status .results').innerText(), /cannot be undone here: the copy downloaded again/);
    assert.equal(await page.evaluate(() => window.__samePage), 'again', 'refused in place, no page load');
    assert.match(await old.innerText(), /Accepted: JE \d+/, 'the old row keeps its entry');
    assert.deepEqual(await reversals(), register, 'nothing was voided');
    await shot('7-undo-refused');
    pass('after a reconnect, Undo on the old row a new copy is linked to is not offered, says why, and is refused when pressed anyway');

    await page.goto(`fb://app/c/${fixture.company_id}/bank-feeds/review/?status=ALL`);
    assert.match(await page.locator('main').innerText(), /Synthetic office supplies/);
    assert.deepEqual(errors, [], `page errors: ${errors.join('; ')}`);
    pass('no page errors');
  } catch (error) {
    await page.screenshot({path: path.join(shots, 'review-speed-failure.png')}).catch(() => {});
    throw error;
  } finally {
    await app.close();
  }
})().catch(e => { console.error(e); process.exitCode = 1; });
