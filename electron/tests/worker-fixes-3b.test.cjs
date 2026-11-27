'use strict';
// Bookkeeper round 7's missing bank-feed features (internal test notes, fix list items 46-48): the page
// behaviour of a split row in Review and of a deposit of several received payments from one Review row, in a
// real sandboxed Electron window with the real preload, page script and stylesheets. The main process side is
// a stand-in for `books.run` that records each command and answers the review page with a prepared fragment.
// Hidden window, headless platform: no display is used. Clicks and keys are real input events (DevTools
// protocol). The markup is the server's (bankfeeds/review_row.html), cut down to the rows under test; what
// the server draws and decides is tested in crates/fb-app/tests/worker_round3b.rs.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { pathToFileURL } = require('node:url');

const root = path.resolve(__dirname, '..');
const asset = (rel) => pathToFileURL(path.join(root, 'renderer', 'static', rel)).href;
const head = `<meta charset="utf-8"><link rel="stylesheet" href="${asset('ledger/app.css')}"><link rel="stylesheet" href="${asset('bankfeeds/bankfeeds.css')}">`;

const CATEGORIES = [['a6140', '6140 · Utilities'], ['a3200', '3200 · Owner\'s Draw'], ['a6100', '6100 · Supplies'], ['a6160', '6160 · Bank Service Charges']];
const options = (chosen) => `<option value="">— choose —</option>${CATEGORIES.map(([v, t]) => `<option value="${v}"${v === chosen ? ' selected' : ''}>${t}</option>`).join('')}`;
// Round 7's TX-0002 (-113.09, Fernvale Fiber), a check whose rule chose 6140 Utilities: as review_row.html
// draws a NEW row before anything is typed (Split not ticked, so no lines yet).
const splitRow = (pk) => `<tr id="review-row-${pk}" data-fragment data-review-row="${pk}" data-status="NEW" tabindex="-1" data-draft-retained="false" data-bulk-block="" data-matching-rules=""><td><input type="checkbox" name="sel" value="${pk}"></td><td class="num">2026-06-05</td><td class="acct">Checking</td><td class="desc">FERNVALE FIBER ACH</td><td class="num out">-113.09</td>
<td class="cell-pick"><select name="kind_${pk}" aria-label="Record FERNVALE FIBER ACH as"><option value="CHECK" selected>Check or debit</option><option value="JOURNAL">Journal entry</option></select></td>
<td class="cell-cat"><span class="for-kind" data-kinds="CHECK JOURNAL"><span class="single-cat"><select name="account_${pk}" data-combobox aria-label="Category for FERNVALE FIBER ACH">${options('a6140')}</select></span><span class="split" data-split data-amount="113.09"><label class="split-toggle" title="Record this transaction over several accounts"><input type="checkbox" name="split_${pk}" value="1" data-split-toggle> Split</label><span class="split-lines" data-split-lines hidden><button type="button" class="split-add" data-split-add>Add line</button><output class="split-left" data-split-left aria-live="polite"></output><span class="row-error-text split-error" role="alert" data-split-error hidden></span></span></span></span></td>
<td class="cell-detail"><span class="for-kind" data-kinds="CHECK JOURNAL"><input type="text" name="payee_${pk}" value="" maxlength="120" placeholder="Payee"></span><span class="for-kind" data-kinds="CHECK"><input type="text" name="number_${pk}" value="" maxlength="20"></span></td>
<td class="row-buttons"><div class="row-actions"><button type="submit" name="accept_one" value="${pk}" class="primary" data-primary id="accept-${pk}">Accept</button><button type="submit" name="ignore_one" value="${pk}" data-ignore>Ignore</button></div></td></tr>`;
// Round 7's processor payout (TX-0057, 825.05) set to "Deposit received payments": two payments waiting
// (850.00 and 300.00), none of the bank's amount, so none ticked; the fee line starts on bank charges.
const depositRow = (pk) => `<tr id="review-row-${pk}" data-fragment data-review-row="${pk}" data-status="NEW" tabindex="-1" data-draft-retained="false" data-bulk-block="" data-matching-rules=""><td><input type="checkbox" name="sel" value="${pk}"></td><td class="num">2026-08-12</td><td class="acct">Checking</td><td class="desc">CARD PROCESSOR PAYOUT</td><td class="num in">825.05</td>
<td class="cell-pick"><select name="kind_${pk}" aria-label="Record CARD PROCESSOR PAYOUT as"><option value="DEPOSIT" selected>Deposit received payments</option><option value="JOURNAL">Deposit (journal)</option></select></td>
<td class="cell-cat"><span class="for-kind" data-kinds="CHECK JOURNAL" hidden><span class="single-cat"><select name="account_${pk}" data-combobox aria-label="Category for CARD PROCESSOR PAYOUT">${options('')}</select></span></span><span class="for-kind deposit-pick" data-kinds="DEPOSIT" data-deposit data-amount="825.05" data-default="a6160"><span class="deposit-title">Payments in this deposit</span><label class="deposit-receipt"><input type="checkbox" name="receipt_${pk}" value="r8" data-amount="850.00" id="pick-r8"> <span>RCP-000008 Old Mill Insurance Agency<span class="muted"> · 2026-08-12 · 850.00</span></span></label><label class="deposit-receipt"><input type="checkbox" name="receipt_${pk}" value="r9" data-amount="300.00" id="pick-r9"> <span>RCP-000009 Copperleaf Dental Studio<span class="muted"> · 2026-08-10 · 300.00</span></span></label><span class="deposit-title">Fees and adjustments</span><span class="deposit-fees" data-fee-rows><span class="fee-line" data-fee-line><select name="fee_account_${pk}" aria-label="Account, fee or adjustment line 1">${options('a6160')}</select><input type="text" name="fee_amount_${pk}" value="" inputmode="decimal" maxlength="20" autocomplete="off" placeholder="Fee, such as -24.95" aria-label="Amount, fee or adjustment line 1"></span></span><button type="button" class="split-add" data-fee-add>Add fee line</button><output class="split-left" data-deposit-left aria-live="polite"></output><span class="row-error-text split-error" role="alert" data-deposit-error hidden></span></span></td>
<td class="cell-detail"><span class="for-kind" data-kinds="CHECK JOURNAL" hidden><input type="text" name="payee_${pk}" value="" maxlength="120" placeholder="Payee"></span></td>
<td class="row-buttons"><div class="row-actions"><button type="submit" name="accept_one" value="${pk}" class="primary" data-primary id="accept-${pk}">Accept</button><button type="submit" name="ignore_one" value="${pk}" data-ignore>Ignore</button></div></td></tr>`;
const acceptedRow = (pk, reference) => `<tr id="review-row-${pk}" data-fragment data-review-row="${pk}" data-status="ACCEPTED" tabindex="-1" data-draft-retained="false" data-bulk-block="" data-matching-rules=""><td></td><td class="num">2026-06-05</td><td class="acct">Checking</td><td class="desc">ROW ${pk}</td><td class="num">-</td><td colspan="3" class="outcome">Accepted: ${reference}</td><td></td></tr>`;
const status = (n, text = '') => `<div id="review-status" data-fragment data-to-review="${n}"><p class="review-count"><strong>${n} to review</strong></p>${text ? `<ul class="results"><li>${text}</li></ul>` : ''}</div>`;
const reviewPage = (rows) => `<!doctype html><html><head>${head}</head><body class="shell"><div class="bankfeed review-page">
<form method="post" id="bankfeed-review" action="/c/00000000-0000-0000-0000-000000000001/bank-feeds/review/"><input type="hidden" name="fb_command_token" value="t-review"><input type="hidden" name="review_layout" value="new">
${status(1)}<div id="review-live" class="sr-only" role="status" aria-live="polite"></div>
<table class="review review-new"><colgroup><col class="c-pick"><col class="c-date"><col class="c-acct"><col class="c-desc"><col class="c-amt"><col class="c-kind"><col class="c-cat"><col class="c-detail"><col class="c-act"></colgroup><tbody>${rows}</tbody></table></form></div>
<script src="${asset('bankfeeds/review.js')}"></script></body></html>`;
const fragment = (html) => ({ id: /id="([^"]+)"/.exec(html)[1], html });
// The stand-in server's in-place replies, in order.
const REPLIES = [
  { fragments: [status(0, '2026-06-05 FERNVALE FIBER ACH -113.09: recorded as CHK-000021, split over 2 lines'), acceptedRow(1, 'CHK-000021')].map(fragment), next_token: 't-review-2' },
  { fragments: [status(0, '2026-08-12 CARD PROCESSOR PAYOUT 825.05: recorded as DEP-000005'), acceptedRow(2, 'DEP-000005')].map(fragment), next_token: 't-review-3' },
];
// Record deposit (banking/deposits/form.html with banking/fee_lines.html): two payments waiting, the fee
// line starting on the bank-charges account.
const feeOptions = (chosen) => `<option value="">Choose…</option>${[['a6160', '6160 · Bank Service Charges'], ['a6020', '6020 · Commissions and Fees']].map(([v, t]) => `<option value="${v}"${v === chosen ? ' selected' : ''}>${t}</option>`).join('')}`;
const depositPage = `<!doctype html><html><head>${head}<link rel="stylesheet" href="${asset('banking/deposits.css')}"><link rel="stylesheet" href="${asset('banking/fees.css')}"></head><body><div class="deposit"><form method="post" id="deposit-entry">
<table class="receipt-choices"><tbody><tr><td><input type="checkbox" name="receipts" value="r8" id="receipt-r8" data-amount="850.00"></td><td>RCP-000008</td></tr><tr><td><input type="checkbox" name="receipts" value="r9" id="receipt-r9" data-amount="300.00"></td><td>RCP-000009</td></tr></tbody></table>
<fieldset class="fee-lines" id="fee-lines" data-fees data-base-from="receipts" data-base-label="Payments" data-total-label="deposit total" data-default="a6160"><legend>Fees and adjustments</legend><p class="fee-hint" id="fee-hint">A fee is negative, such as -24.95.</p>
<table class="fee-table"><tbody data-fee-rows><tr data-fee-line><td><select name="fee_account" aria-label="Account, fee or adjustment line 1">${feeOptions('a6160')}</select></td><td class="num"><input type="text" name="fee_amount" value="" inputmode="decimal" maxlength="20" autocomplete="off" aria-label="Amount, fee or adjustment line 1"></td><td><button type="button" class="fee-remove" data-fee-remove aria-label="Remove fee or adjustment line 1">Remove</button></td></tr></tbody></table>
<p class="fee-foot"><button type="button" data-fee-add>Add a line</button> <output class="fee-total" data-fee-total aria-live="polite"></output></p></fieldset></form></div>
<script src="${asset('banking/fees.js')}"></script></body></html>`;
const PAGES = { split: reviewPage(splitRow(1)), deposit: depositPage, pick: reviewPage(depositRow(2)) };

const MAIN = `'use strict';
const { app, BrowserWindow, ipcMain } = require('electron');
const pages = JSON.parse(process.env.FB_TEST_PAGES), preload = process.env.FB_TEST_PRELOAD, replies = JSON.parse(process.env.FB_TEST_REPLIES);
const commands = [];
const out = (v) => { process.stdout.write('RESULT ' + JSON.stringify(v) + '\\n'); app.exit(0); };
ipcMain.handle('fb:books.run', (_e, command) => {
  commands.push(command);
  if (command.path && command.path.endsWith('/bank-feeds/review/')) return { ok: true, value: { outcome: 'done', data: replies.shift() || { fragments: [], next_token: 'none' } } };
  return { ok: true, value: { outcome: 'done', data: {} } };
});
const sleep = (ms) => new Promise(r => setTimeout(r, ms));
app.whenReady().then(async () => {
  const win = new BrowserWindow({ show: false, width: 1440, height: 900, webPreferences: { preload, sandbox: true, contextIsolation: true, nodeIntegration: false, backgroundThrottling: false } });
  const js = (code) => win.webContents.executeJavaScript(code);
  const until = async (code) => { for (let i = 0; i < 400; i++) { if (await js(code)) return; await sleep(25); } throw new Error('timed out: ' + code); };
  const cdp = win.webContents.debugger;
  const send = (method, params) => cdp.sendCommand(method, params);
  const load = async (name) => {
    await win.loadFile(pages[name]);
    if (!cdp.isAttached()) {
      cdp.attach('1.3');
      await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
    }
    await until('document.readyState === "complete"');
  };
  const click = async (selector) => {
    const r = await js(\`(() => { const el = document.querySelector(\${JSON.stringify(selector)}); el.scrollIntoView({ block: 'center' }); const b = el.getBoundingClientRect(); return { x: Math.round(b.left + Math.min(b.width - 3, b.width / 2 + 2)), y: Math.round(b.top + b.height / 2) }; })()\`);
    await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: r.x, y: r.y, button: 'left', buttons: 1, clickCount: 1 });
    await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: r.x, y: r.y, button: 'left', buttons: 0, clickCount: 1 });
    await sleep(80);
  };
  const type = async (text) => {
    for (const ch of text) {
      await send('Input.dispatchKeyEvent', { type: 'keyDown', text: ch, key: ch, unmodifiedText: ch });
      await send('Input.dispatchKeyEvent', { type: 'keyUp', key: ch });
    }
    await sleep(120);
  };
  // Replace a box's text by typing, as a person does after selecting what is there.
  const retype = async (selector, text) => { await click(selector); await js(\`document.querySelector(\${JSON.stringify(selector)}).select(); true\`); await type(text); };
  const choose = (selector, value) => js(\`(() => { const s = document.querySelector(\${JSON.stringify(selector)}); s.value = \${JSON.stringify(value)}; s.dispatchEvent(new Event('change', { bubbles: true })); return true; })()\`);
  const result = { commands };
  try {
    win.webContents.focus();
    // Item 46: Split on round 7's Fernvale Fiber row, whose rule chose 6140 Utilities.
    await load('split');
    await js("window.__fragments = 0; document.getElementById('bankfeed-review').addEventListener('fb:fragments', () => { window.__fragments++; }); true");
    const lines = () => js("[...document.querySelectorAll('#review-row-1 [data-split-line]')].map(l => ({ account: l.querySelector('select').value, amount: l.querySelector('[data-split-amount]').value, memo: l.querySelector('[name=split_memo_1]').value, label: l.querySelector('.combo-input')?.value }))");
    const left = () => js("(() => { const o = document.querySelector('#review-row-1 [data-split-left]'); return { text: o.textContent, cls: o.className }; })()");
    const message = () => js("(() => { const m = document.querySelector('#review-row-1 [data-split-error]'); return { hidden: m.hidden, text: m.textContent }; })()");
    await click('#review-row-1 [data-split-toggle]');
    result.ticked = {
      lines: await lines(), left: await left(),
      single: await js("document.querySelector('#review-row-1 .single-cat').hidden"),
      shown: await js("!document.querySelector('#review-row-1 [data-split-lines]').hidden"),
      focus: await js("document.activeElement.getAttribute('aria-label')"),
    };
    await type('93.10');
    result.afterFirst = await left();
    await choose('#review-row-1 [data-split-line]:nth-of-type(2) select', 'a3200');
    await retype('#review-row-1 [data-split-line]:nth-of-type(2) [data-split-amount]', '21.00');
    result.over = await left();
    await retype('#review-row-1 [data-split-line]:nth-of-type(2) [data-split-amount]', '10.00');
    result.short = await left();
    // Accept while 9.99 is left: refused at the lines, nothing sent.
    const sentBefore = commands.length;
    await click('#accept-1');
    await sleep(300);
    result.refused = { sent: commands.length - sentBefore, message: await message(), focus: await js("document.activeElement.getAttribute('aria-label')"), status: await js("document.querySelector('#review-row-1').dataset.status") };
    // Add a third line, then remove it again.
    await click('#review-row-1 [data-split-add]');
    result.added = (await lines()).length;
    await click('#review-row-1 [data-split-line]:nth-of-type(3) [data-split-remove]');
    result.removed = (await lines()).length;
    // Corrected: 0.00 left, and Accept posts the lines.
    await retype('#review-row-1 [data-split-line]:nth-of-type(2) [data-split-amount]', '19.99');
    await click('#review-row-1 [data-split-line]:nth-of-type(2) [name=split_memo_1]');
    await type('Home TV add-on');
    result.balanced = { left: await left(), lines: await lines() };
    await click('#accept-1');
    await until('window.__fragments === 1');
    result.sent = commands.at(-1).fields.filter(([k]) => /^(split|kind|account|accept_one)/.test(k));
    result.after = await js("document.querySelector('#review-row-1').dataset.status");

    // Item 47: Record deposit with a fee line: the total follows the ticks and the fee, live.
    await load('deposit');
    const total = () => js("document.querySelector('[data-fee-total]').textContent");
    await click('#receipt-r8');
    result.feeNone = await total();
    await click('[name=fee_amount]');
    await type('-24.95');
    result.feeOne = await total();
    await click('#receipt-r9');
    result.feeTwo = await total();
    await click('[data-fee-add]');
    result.feeAdded = await js("[...document.querySelectorAll('[data-fee-line]')].map(l => [l.querySelector('select').value, l.querySelector('input').value, l.querySelector('input').getAttribute('aria-label')])");
    await click('[data-fee-line]:nth-of-type(2) [name=fee_amount]');
    await type('abc');
    result.feeBad = await total();
    await click('[data-fee-line]:nth-of-type(2) [data-fee-remove]');
    result.feeRemoved = { lines: await js("document.querySelectorAll('[data-fee-line]').length"), total: await total() };

    // Item 48: the payout's Review row deposits the 850.00 payment with its -24.95 fee.
    await load('pick');
    await js("window.__fragments = 0; document.getElementById('bankfeed-review').addEventListener('fb:fragments', () => { window.__fragments++; }); true");
    const pickLeft = () => js("(() => { const o = document.querySelector('#review-row-2 [data-deposit-left]'); return { text: o.textContent, cls: o.className }; })()");
    const pickMessage = () => js("(() => { const m = document.querySelector('#review-row-2 [data-deposit-error]'); return { hidden: m.hidden, text: m.textContent }; })()");
    result.pickStart = await pickLeft();
    let sent = commands.length;
    await click('#accept-2');
    await sleep(300);
    result.pickNone = { sent: commands.length - sent, message: await pickMessage() };
    await click('#pick-r8');
    result.pickOver = await pickLeft();
    sent = commands.length;
    await click('#accept-2');
    await sleep(300);
    result.pickRefused = { sent: commands.length - sent, message: await pickMessage(), status: await js("document.querySelector('#review-row-2').dataset.status") };
    await click('#review-row-2 [name=fee_amount_2]');
    await type('-24.95');
    result.pickEqual = await pickLeft();
    await click('#review-row-2 [data-fee-add]');
    result.pickLines = await js("[...document.querySelectorAll('#review-row-2 [data-fee-line]')].map(l => [l.querySelector('select').value, l.querySelector('input').value])");
    await click('#accept-2');
    await until('window.__fragments === 1');
    result.pickSent = commands.at(-1).fields.filter(([k]) => /^(kind|receipt|fee|accept_one)/.test(k));
    result.pickAfter = await js("document.querySelector('#review-row-2').dataset.status");
    out(result);
  } catch (e) { out({ error: String(e && e.stack || e), ...result }); }
});
`;

test('worker round 7 bank-feed features: a split row in Review, fee lines on a deposit, a deposit of received payments from a Review row', { timeout: 300000 }, async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-worker-fixes-3b-'));
  try {
    const pages = {};
    for (const [name, html] of Object.entries(PAGES)) {
      pages[name] = path.join(dir, `${name}.html`);
      fs.writeFileSync(pages[name], html);
    }
    const main = path.join(dir, 'main.cjs');
    fs.writeFileSync(main, MAIN);
    const electron = require(path.join(root, 'node_modules', 'electron'));
    const env = { ...process.env, FB_TEST_PAGES: JSON.stringify(pages), FB_TEST_PRELOAD: path.join(root, 'preload', 'preload.cjs'), FB_TEST_REPLIES: JSON.stringify(REPLIES), ELECTRON_ENABLE_LOGGING: '0' };
    delete env.ELECTRON_RUN_AS_NODE;
    const output = await new Promise((resolve, reject) => {
      const child = spawn(electron, ['--ozone-platform=headless', `--user-data-dir=${path.join(dir, 'profile')}`, main], { env, stdio: ['ignore', 'pipe', 'pipe'] });
      let stdout = '', stderr = '';
      child.stdout.on('data', d => { stdout += d; });
      child.stderr.on('data', d => { stderr += d; });
      const timer = setTimeout(() => { child.kill('SIGKILL'); reject(new Error(`Electron timed out\n${stderr}`)); }, 260000);
      child.on('exit', () => { clearTimeout(timer); resolve({ stdout, stderr }); });
    });
    const line = output.stdout.split('\n').find(l => l.startsWith('RESULT '));
    assert.ok(line, `no result from Electron:\n${output.stdout}\n${output.stderr}`);
    const r = JSON.parse(line.slice(7));
    assert.equal(r.error, undefined, r.error);

    // Item 46: ticking Split makes two lines; the rule's category starts the first; the one category box
    // gives way to the lines; the first amount has the focus; the remaining amount is shown.
    assert.deepEqual(r.ticked.lines, [
      { account: 'a6140', amount: '', memo: '', label: '6140 · Utilities' },
      { account: '', amount: '', memo: '', label: '' },
    ]);
    assert.equal(r.ticked.single, true);
    assert.equal(r.ticked.shown, true);
    assert.equal(r.ticked.focus, 'Amount, split line 1');
    assert.deepEqual(r.ticked.left, { text: 'Enter an amount on each line', cls: 'split-left over' });
    // The remaining amount, live, as amounts are typed.
    assert.deepEqual(r.afterFirst, { text: 'Remaining 19.99', cls: 'split-left short' });
    assert.deepEqual(r.over, { text: 'Over by 1.01', cls: 'split-left over' });
    assert.deepEqual(r.short, { text: 'Remaining 9.99', cls: 'split-left short' });
    // A split that does not add up is refused at its lines: nothing is sent, the row stays to review.
    assert.deepEqual(r.refused, {
      sent: 0,
      message: { hidden: false, text: "The split lines add up to 103.10, not the bank's 113.09: 9.99 is left to assign. Nothing was recorded." },
      focus: 'Amount, split line 1',
      status: 'NEW',
    });
    assert.equal(r.added, 3);
    assert.equal(r.removed, 2);
    // Corrected to 0.00 left: Accept sends Split and every line in order.
    assert.deepEqual(r.balanced.left, { text: 'Remaining 0.00', cls: 'split-left ok' });
    assert.deepEqual(r.sent, [
      ['kind_1', 'CHECK'], ['account_1', 'a6140'], ['split_1', '1'],
      ['split_account_1', 'a6140'], ['split_amount_1', '93.10'], ['split_memo_1', ''],
      ['split_account_1', 'a3200'], ['split_amount_1', '19.99'], ['split_memo_1', 'Home TV add-on'],
      ['accept_one', '1'],
    ]);
    assert.equal(r.after, 'ACCEPTED');

    // Item 47: the deposit's fee lines and its live total.
    assert.equal(r.feeNone, 'deposit total 850.00', 'selected receipts count before any fees');
    assert.equal(r.feeOne, 'Payments 850.00 · fees and adjustments -24.95 · deposit total 825.05');
    assert.equal(r.feeTwo, 'Payments 1,150.00 · fees and adjustments -24.95 · deposit total 1,125.05');
    assert.deepEqual(r.feeAdded, [['a6160', '-24.95', 'Amount, fee or adjustment line 1'], ['a6160', '', 'Amount, fee or adjustment line 2']]);
    assert.equal(r.feeBad, 'Enter each fee or adjustment as an amount such as -24.95.');
    assert.deepEqual(r.feeRemoved, { lines: 1, total: 'Payments 1,150.00 · fees and adjustments -24.95 · deposit total 1,125.05' });

    // Item 48: the deposit pick on a Review row, its live total, and its refusals at the deposit lines.
    assert.deepEqual(r.pickStart, { text: "Payments 0.00: 825.05 short of the bank's 825.05", cls: 'split-left short' });
    assert.deepEqual(r.pickNone, { sent: 0, message: { hidden: false, text: 'Tick the received payments in this bank deposit.' } });
    assert.deepEqual(r.pickOver, { text: "Payments 850.00: 24.95 more than the bank's 825.05", cls: 'split-left over' });
    // Ticks that do not equal the bank amount are refused: nothing is sent.
    assert.deepEqual(r.pickRefused, { sent: 0, message: { hidden: false, text: "The selected receipts total 850.00, not the bank's 825.05." }, status: 'NEW' });
    assert.deepEqual(r.pickEqual, { text: 'Payments 850.00 · fees -24.95 = bank 825.05', cls: 'split-left ok' });
    assert.deepEqual(r.pickLines, [['a6160', '-24.95'], ['a6160', '']]);
    assert.deepEqual(r.pickSent, [
      ['kind_2', 'DEPOSIT'], ['receipt_2', 'r8'],
      ['fee_account_2', 'a6160'], ['fee_amount_2', '-24.95'], ['fee_account_2', 'a6160'], ['fee_amount_2', ''],
      ['accept_one', '2'],
    ]);
    assert.equal(r.pickAfter, 'ACCEPTED');
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
