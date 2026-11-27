'use strict';
// Review of the round 7 bank-feed features (internal test notes, M1 and M7): the fee and
// adjustment lines on Receive payment, in a real sandboxed Electron window with the real preload, page scripts
// (sales/receipts.js, banking/fees.js) and stylesheets. The main process is a stand-in for `books.run` that
// records each command. Hidden window, headless platform: no display is used. Clicks and keys are real input
// events (DevTools protocol). The markup is the server's (sales/receipts/form.html with banking/fee_lines.html);
// what the server warns about and records is tested in crates/fb-app/tests/worker_round3b.rs.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { pathToFileURL } = require('node:url');

const root = path.resolve(__dirname, '..');
const asset = (rel) => pathToFileURL(path.join(root, 'renderer', 'static', rel)).href;

// The fee accounts as fees::choices draws them: expense accounts carry `data-expense`.
const FEE_ACCOUNTS = [['a6160', '6160 · Bank Service Charges', true], ['a6020', '6020 · Commissions and Fees', true], ['a4900', '4900 · Other Income', false]];
const feeOptions = (chosen) => `<option value="">Choose…</option>${FEE_ACCOUNTS.map(([v, t, expense]) => `<option value="${v}"${expense ? ' data-expense' : ''}${v === chosen ? ' selected' : ''}>${t}</option>`).join('')}`;
const feeBox = ({ hidden, amount = '', warning = '' }) => `<fieldset class="fee-lines" id="fee-lines" data-fees data-base-from="amount" data-base-label="Received" data-total-label="the bank gets" data-default="a6160"${hidden ? ' hidden disabled' : ''}><legend>Fees and adjustments</legend>
<p class="fee-hint" id="fee-hint">What the card processor or bank kept. A fee is negative, such as -24.95; leave the amount empty for none.</p>${warning}
<table class="fee-table"><tbody data-fee-rows><tr data-fee-line><td><select name="fee_account" aria-label="Account, fee or adjustment line 1">${feeOptions('a6160')}</select></td><td class="num"><input type="text" name="fee_amount" value="${amount}" inputmode="decimal" maxlength="20" autocomplete="off" aria-label="Amount, fee or adjustment line 1"></td><td><button type="button" class="fee-remove" data-fee-remove aria-label="Remove fee or adjustment line 1">Remove</button></td></tr></tbody></table>
<p class="fee-foot"><button type="button" data-fee-add>Add a line</button> <output class="fee-total" data-fee-total aria-live="polite"></output></p></fieldset>`;
// The server's M1 warning, as fee_lines.html draws it after a first Save with a positive fee on 6160.
const WARNING = '24.95 on 6160 · Bank Service Charges is positive, so it will increase the deposit by 24.95 and credit 6160 · Bank Service Charges, an expense account. A fee the bank or card processor kept is negative, such as -24.95. If the amount is right, choose Save payment again; otherwise put a minus sign before it.';
const warned = `<p class="notice fee-warning" role="alert" id="fee-warning" data-fee-warning><strong>Nothing was saved yet.</strong> ${WARNING}</p><input type="hidden" name="fee_ok" value="a6160=24.95" data-fee-ok>`;
// Receive payment for round 7's payout: Old Mill's INV-001007 (850.00) paid in full.
const receive = ({ bank, fees }) => `<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="${asset('sales/sales-pages.css')}"><link rel="stylesheet" href="${asset('banking/fees.css')}"></head><body><div class="receive-payment">
<form method="post" id="receive-payment" action="/c/00000000-0000-0000-0000-000000000001/receipts/new/" novalidate><input type="hidden" name="fb_command_token" value="t-receive"><input type="hidden" name="request_key" value="7d3f1c2e-0b6a-4c55-9a51-2f4e8d9c1a07">
<section class="payment-fields"><div class="field field-customer"><select name="customer" id="id_customer"><option value="">Choose…</option><option value="oldmill" selected>Old Mill Insurance Agency (TEST)</option></select></div>
<div class="field field-amount"><label for="id_amount">Amount received:</label><input type="text" name="amount" inputmode="decimal" maxlength="32" value="850.00" required id="id_amount"><p class="notice amount-extra" id="amount-extra" role="status" hidden></p></div>
<div class="field"><select name="destination" id="id_destination"><option value="UNDEPOSITED"${bank ? '' : ' selected'}>Undeposited funds</option><option value="BANK"${bank ? ' selected' : ''}>Bank account</option></select></div>
<div class="field" data-bank-field${bank ? '' : ' hidden'}><select name="bank" id="id_bank"><option value="">Choose…</option><option value="b1000"${bank ? ' selected' : ''}>1000 · Checking</option></select></div></section>
${fees}
<section class="open-invoices"><p class="apply-summary" data-summary aria-live="polite"></p><input type="hidden" name="allocations-TOTAL_FORMS" id="id_allocations-TOTAL_FORMS" value="1"><p class="apply-empty" data-empty hidden></p>
<table class="apply-table" data-table><tbody id="invoice-rows"><tr data-invoice="inv7" data-balance="850.00" data-receivables="1100 · Accounts Receivable" data-chosen="1"><td class="pay"><input type="hidden" name="allocations-0-invoice" value="inv7"><input type="checkbox" name="allocations-0-pay" id="id_allocations-0-pay" checked aria-label="Pay INV-001007"></td><th scope="row">INV-001007</th><td class="num">850.00</td><td class="num apply"><input type="text" name="allocations-0-amount" id="id_allocations-0-amount" inputmode="decimal" maxlength="32" autocomplete="off" value="850.00" aria-label="Amount to apply to INV-001007"></td></tr></tbody><tfoot><tr><th colspan="3">Applied</th><td class="num" data-applied>—</td></tr></tfoot></table></section>
<details class="entries-panel" open><summary>Show accounting entries</summary><p>When you save, this payment posts:</p><table><thead><tr><th>Account</th><th class="num">Debit</th><th class="num">Credit</th></tr></thead><tbody data-entries></tbody></table></details>
<button type="submit" name="action" value="save" id="save">Save payment</button></form>
<script type="application/json" id="open-invoices">[{"invoice_id":"inv7","customer_id":"oldmill","number":"INV-001007","date":"2026-08-04","due_date":"2026-08-04","total":"850.00","outstanding":"850.00","overdue":false,"receivables":"1100 · Accounts Receivable"}]</script>
<script type="application/json" id="payment-accounts">{"undeposited":"1200 · Undeposited Funds","banks":{}}</script>
<script src="${asset('sales/receipts.js')}"></script><script src="${asset('banking/fees.js')}"></script></div></body></html>`;
const PAGES = {
  // As first drawn: kept in Undeposited Funds, so the fee lines are hidden (and disabled).
  receive: receive({ bank: false, fees: feeBox({ hidden: true }) }),
  // A first Save straight to Checking with 24.95 (no minus sign) on 6160, shown again with the server's warning.
  warned: receive({ bank: true, fees: feeBox({ hidden: false, amount: '24.95', warning: warned }) }),
};

const MAIN = `'use strict';
const { app, BrowserWindow, ipcMain } = require('electron');
const pages = JSON.parse(process.env.FB_TEST_PAGES), preload = process.env.FB_TEST_PRELOAD;
const commands = [];
const out = (v) => { process.stdout.write('RESULT ' + JSON.stringify(v) + '\\n'); app.exit(0); };
ipcMain.handle('fb:books.run', (_e, command) => { commands.push(command); return { ok: true, value: { outcome: 'done', data: {} } }; });
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
  const retype = async (selector, text) => { await click(selector); await js(\`document.querySelector(\${JSON.stringify(selector)}).select(); true\`); await type(text); };
  const choose = (selector, value) => js(\`(() => { const s = document.querySelector(\${JSON.stringify(selector)}); s.value = \${JSON.stringify(value)}; s.dispatchEvent(new Event('change', { bubbles: true })); return true; })()\`);
  const box = () => js("(() => { const b = document.querySelector('[data-fees]'); return { hidden: b.hidden, disabled: b.disabled, shown: b.getBoundingClientRect().height > 0 }; })()");
  const total = () => js("(() => { const o = document.querySelector('[data-fee-total]'); return { text: o.textContent, attention: o.classList.contains('attention') }; })()");
  const entries = () => js("[...document.querySelectorAll('[data-entries] tr')].map(tr => [...tr.children].map(c => c.textContent).filter(Boolean).join(' | '))");
  const save = async () => {
    const before = commands.length;
    await click('#save');
    for (let i = 0; i < 200 && commands.length === before; i++) await sleep(25);
    return commands.length > before ? commands.at(-1).fields.filter(([k]) => /^(destination|bank|fee_)/.test(k)) : null;
  };
  const result = { commands };
  try {
    win.webContents.focus();
    // M7: as first drawn, the fee lines are hidden and disabled; the preview has one entry.
    await load('receive');
    result.start = { box: await box(), entries: await entries() };
    // Straight to Checking: the fee lines are shown, and with a fee the preview shows both entries.
    await choose('#id_destination', 'BANK');
    await choose('#id_bank', 'b1000');
    result.bank = await box();
    await click('[name=fee_amount]');
    await type('-24.95');
    result.net = { total: await total(), entries: await entries() };
    // M1: the same fee without its minus sign on 6160, an expense account, is said beside the total.
    await retype('[name=fee_amount]', '24.95');
    result.positive = { total: await total(), entries: await entries() };
    await click('[data-fee-add]');
    await choose('[data-fee-line]:nth-of-type(2) select', 'a6020');
    await click('[data-fee-line]:nth-of-type(2) [name=fee_amount]');
    await type('3.00');
    result.positiveTwo = await total();
    // A positive line on an income account is an adjustment: not said.
    await choose('[data-fee-line]:nth-of-type(2) select', 'a4900');
    result.income = await total();
    await click('[data-fee-line]:nth-of-type(2) [data-fee-remove]');
    // M7: a comma is refused live, as the save refuses it.
    await retype('[name=fee_amount]', '1,024.95');
    result.comma = { total: await total(), entries: await entries() };
    await retype('[name=fee_amount]', '-24.95');
    // M7: back to Undeposited Funds: the lines are hidden and disabled, not sent, and the preview has one entry.
    await choose('#id_destination', 'UNDEPOSITED');
    result.held = { box: await box(), entries: await entries() };
    result.heldSent = await save();
    // Straight to the bank again: what was typed is back, and sent.
    await choose('#id_destination', 'BANK');
    result.again = { box: await box(), amount: await js("document.querySelector('[name=fee_amount]').value"), total: await total() };
    result.againSent = await save();

    // M1: after the server's warning, Save payment again sends its confirmation; a changed line drops it.
    await load('warned');
    result.warned = { box: await box(), total: await total(), warning: await js("document.querySelector('[data-fee-warning]')?.textContent || ''") };
    result.warnedSent = await save();
    await load('warned');
    await retype('[name=fee_amount]', '-24.95');
    result.changed = { warning: await js("!!document.querySelector('[data-fee-warning]')"), ok: await js("!!document.querySelector('[name=fee_ok]')"), total: await total() };
    result.changedSent = await save();
    out(result);
  } catch (e) { out({ error: String(e && e.stack || e), ...result }); }
});
`;

test('review of round 7 features: fee lines on Receive payment warn when positive on an expense, are not sent while hidden, preview both entries and refuse commas', { timeout: 300000 }, async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-worker-fixes-3b-review-'));
  try {
    const pages = {};
    for (const [name, html] of Object.entries(PAGES)) {
      pages[name] = path.join(dir, `${name}.html`);
      fs.writeFileSync(pages[name], html);
    }
    const main = path.join(dir, 'main.cjs');
    fs.writeFileSync(main, MAIN);
    const electron = require(path.join(root, 'node_modules', 'electron'));
    const env = { ...process.env, FB_TEST_PAGES: JSON.stringify(pages), FB_TEST_PRELOAD: path.join(root, 'preload', 'preload.cjs'), ELECTRON_ENABLE_LOGGING: '0' };
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

    // M7: kept in Undeposited Funds, the fee lines are hidden and disabled; one entry is previewed.
    assert.deepEqual(r.start.box, { hidden: true, disabled: true, shown: false });
    assert.deepEqual(r.start.entries, ['1200 · Undeposited Funds | 850.00', '1100 · Accounts Receivable | 850.00']);
    assert.deepEqual(r.bank, { hidden: false, disabled: false, shown: true });
    // M7: with a fee, both entries: the payment held in Undeposited Funds, then the deposit net of the fee.
    assert.deepEqual(r.net.total, { text: 'Received 850.00 · fees and adjustments -24.95 · the bank gets 825.05', attention: false });
    assert.deepEqual(r.net.entries, [
      '1. The payment, held in Undeposited Funds',
      '1200 · Undeposited Funds | 850.00',
      '1100 · Accounts Receivable | 850.00',
      '2. The deposit to 1000 · Checking, net of the fees and adjustments',
      '1000 · Checking | 825.05',
      '6160 · Bank Service Charges | 24.95',
      '1200 · Undeposited Funds | 850.00',
    ]);
    // M1: a positive line on an expense account is said, live, with what it does.
    assert.deepEqual(r.positive.total, { text: 'Received 850.00 · fees and adjustments 24.95 · the bank gets 874.95. 24.95 on 6160 · Bank Service Charges is positive, so it adds to the deposit and credits that expense; a fee is negative, such as -24.95.', attention: true });
    assert.deepEqual(r.positive.entries.slice(3), ['2. The deposit to 1000 · Checking, net of the fees and adjustments', '1000 · Checking | 874.95', '6160 · Bank Service Charges | 24.95', '1200 · Undeposited Funds | 850.00']);
    assert.deepEqual(r.positiveTwo, { text: 'Received 850.00 · fees and adjustments 27.95 · the bank gets 877.95. 24.95 on 6160 · Bank Service Charges and 3.00 on 6020 · Commissions and Fees are positive, so they add to the deposit and credit those expenses; a fee is negative, such as -24.95.', attention: true });
    assert.deepEqual(r.income, { text: 'Received 850.00 · fees and adjustments 27.95 · the bank gets 877.95. 24.95 on 6160 · Bank Service Charges is positive, so it adds to the deposit and credits that expense; a fee is negative, such as -24.95.', attention: true });
    // M7: a comma is refused live as the save refuses it; the preview leaves the line out.
    assert.deepEqual(r.comma.total, { text: 'Enter each fee or adjustment as an amount such as -24.95.', attention: true });
    assert.deepEqual(r.comma.entries, ['1000 · Checking | 850.00', '1100 · Accounts Receivable | 850.00']);
    // M7: hidden again, the lines are disabled and not sent; one entry is previewed.
    assert.deepEqual(r.held.box, { hidden: true, disabled: true, shown: false });
    assert.deepEqual(r.held.entries, ['1200 · Undeposited Funds | 850.00', '1100 · Accounts Receivable | 850.00']);
    assert.deepEqual(r.heldSent, [['destination', 'UNDEPOSITED']]);
    // Straight to the bank again: what was typed is back, and sent.
    assert.deepEqual(r.again.box, { hidden: false, disabled: false, shown: true });
    assert.equal(r.again.amount, '-24.95');
    assert.equal(r.again.total.text, 'Received 850.00 · fees and adjustments -24.95 · the bank gets 825.05');
    assert.deepEqual(r.againSent, [['destination', 'BANK'], ['bank', 'b1000'], ['fee_account', 'a6160'], ['fee_amount', '-24.95']]);

    // M1: the server's warning at the lines; Save payment again sends its confirmation.
    assert.deepEqual(r.warned.box, { hidden: false, disabled: false, shown: true });
    assert.equal(r.warned.warning, `Nothing was saved yet. ${WARNING}`);
    assert.equal(r.warned.total.attention, true);
    assert.deepEqual(r.warnedSent, [['destination', 'BANK'], ['bank', 'b1000'], ['fee_ok', 'a6160=24.95'], ['fee_account', 'a6160'], ['fee_amount', '24.95']]);
    // A changed line drops the warning and its confirmation, so the server checks it again.
    assert.deepEqual(r.changed, { warning: false, ok: false, total: { text: 'Received 850.00 · fees and adjustments -24.95 · the bank gets 825.05', attention: false } });
    assert.deepEqual(r.changedSent, [['destination', 'BANK'], ['bank', 'b1000'], ['fee_account', 'a6160'], ['fee_amount', '-24.95']]);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('deposit totals group whole dollars while preserving exact cents', () => {
  const {money} = require('../renderer/static/banking/fees.js');
  assert.equal(money(109188n), '1,091.88');
  assert.equal(money(-150000000000000001n), '-1,500,000,000,000,000.01');
});
