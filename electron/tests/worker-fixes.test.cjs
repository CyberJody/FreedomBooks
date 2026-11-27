'use strict';
// Bookkeeper test rounds of 2026-11-23 (internal test notes): the page behaviour behind items 1, 6, 11,
// 12 and 18 and the review's S1 and N3, in a real sandboxed Electron window with the real preload, page
// scripts and stylesheets. The main process side is a stand-in for `books.run` that records each command. Hidden window, headless
// platform: no display is used. Input events are real (sendInputEvent), not synthetic DOM events.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { pathToFileURL } = require('node:url');

const root = path.resolve(__dirname, '..');
const asset = (rel) => pathToFileURL(path.join(root, 'renderer', 'static', rel)).href;

const journalRow = index => `<tr class="journal-line"><td><label class="sr-only" for="account-${index}">Additional line account</label><select id="account-${index}" name="lines-${index}-account" data-picker="account"><option></option><option value="a">1000 Checking</option></select></td>
<td><input name="lines-${index}-debit"></td><td><input name="lines-${index}-credit"></td><td><input name="lines-${index}-description"></td></tr>`;
const PAGES = {
  journal: `<form class="journal-entry"><input name="memo" value="Year end"><input id="id_lines-TOTAL_FORMS" value="1">
  <table class="journal-lines"><tbody>${journalRow(0)}</tbody></table><template id="empty-line">${journalRow('__prefix__')}</template>
  <button type="button" id="add-line">Add line</button><output id="total-debit"></output><output id="total-credit"></output><output id="total-difference"></output></form>
  <script src="${asset('payables/account-picker.js')}"></script><script src="${asset('ledger/journal.js')}"></script>`,
  refund: `<form id="refund-entry"><select name="bank"><option value="a">A</option><option value="b">B</option></select>
  <select name="method"><option>CHECK</option><option>CASH</option></select><input name="check_number" value="137"></form>
  <script type="application/json" id="refund-numbers">{"a":"137","b":"808"}</script><script src="${asset('banking/refund.js')}"></script>`,
  // Item 1: the first-run gate's step list follows the book password box (markup of desktop/gate.html).
  gate: `<!doctype html><html><head><meta charset="utf-8">${['ledger/app.css','ledger/hybrid.css','desktop/gate.css'].map(css => `<link rel="stylesheet" href="${asset(css)}">`).join('')}</head>
<body class="shell gate-page"><header class="titlebar">FreedomBooks</header><main class="gate"><div class="gate-card">
${fs.readFileSync(path.join(root, '../crates/fb-templates/templates/desktop/gate.html'), 'utf8')
  .split("{% elif mode == 'setup' %}\n")[1].split("{% else %}\n<ol")[0]
  .replace(/\{% if errors %\}[\s\S]*?\{% endif %\}/, '').replace(/\{%.*?%\}|\{\{.*?\}\}/g, '')}
</div></main></body></html>`,
  // Item 11: a saving form with a document date.
  dated: `<!doctype html><html><head><meta charset="utf-8"></head><body>
<form method="post" id="doc" action="/c/00000000-0000-0000-0000-000000000001/invoices/new/"><input type="hidden" name="fb_command_token" value="t1"><input type="hidden" name="request_key" value="k1">
<div class="document-field"><label for="id_date">Date</label><input type="date" name="date" id="id_date" value=""></div>
<button type="submit" id="save">Save</button></form></body></html>`,
  // Review S1: a void form as `form.as_p` prints it, its date in an earlier month than today (the server's
  // today in data-month-notice); and one whose date is in this month.
  voided: `<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="${asset('ledger/app.css')}"></head><body>
<form method="post" class="void-form" action="/c/00000000-0000-0000-0000-000000000001/invoices/00000000-0000-0000-0000-000000000002/void/"><input type="hidden" name="fb_command_token" value="t2">
  <p>
    <label for="id_date">Void date:</label>
    <input type="date" name="date" value="__EARLIER__" data-month-notice="__TODAY__" required id="id_date">
  </p>
  <p>
    <label for="id_reason">Reason:</label>
    <textarea name="reason" cols="40" rows="2" required id="id_reason">Wrong customer</textarea>
  </p>
<button type="submit" id="void">Void invoice</button></form></body></html>`,
  voidedNow: `<!doctype html><html><head><meta charset="utf-8"></head><body>
<form method="post"><p><label for="id_date">Void date:</label><input type="date" name="date" value="__THIS_MONTH__" data-month-notice="__TODAY__" required id="id_date"></p></form></body></html>`,
  // Item 18: prefilled numbers select on focus; other text does not.
  numbers: `<!doctype html><html><head><meta charset="utf-8"><style>input{display:block;width:300px;height:30px;margin:10px;font:16px monospace}</style></head><body>
<form method="post"><input type="text" name="lines-0-quantity" id="qty" value="33.33" inputmode="decimal">
<input type="text" name="lines-0-rate" id="rate" value="150.00">
<input type="text" name="amount" id="amount" value="40.00" inputmode="decimal">
<input type="text" name="memo" id="memo" value="Paid by"></form></body></html>`,
  // Item 6: the sales line editor (invoices.js) and the bill editor (bills.js).
  lines: `<!doctype html><html><head><meta charset="utf-8"></head><body>
<script type="application/json" id="invoice-catalog">{"plain":{"description":"","code":"Custom shelving labor","rate":"65.00","taxable":false,"income_account":"a1"},"described":{"description":"Oak bench, oiled","code":"Oak bench","rate":"425.00","taxable":true,"income_account":"a1"}}</script>
<div class="document-editor invoice-editor"><form method="post"><input type="hidden" name="lines-TOTAL_FORMS" id="id_lines-TOTAL_FORMS" value="1">
<div id="invoice-lines"><fieldset class="invoice-row">
<input type="hidden" name="lines-0-refresh_item" value="">
<div class="line-field" data-field="item"><select name="lines-0-item"><option value=""></option><option value="plain">Custom shelving labor</option><option value="described">Oak bench</option></select><span data-line-kind></span></div>
<div class="line-field" data-field="description"><textarea name="lines-0-description"></textarea></div>
<div class="line-field" data-field="quantity"><input name="lines-0-quantity" value=""></div>
<div class="line-field" data-field="rate"><input name="lines-0-rate" value=""></div>
<div class="line-field" data-field="taxable"><input type="checkbox" name="lines-0-taxable"></div>
<div class="line-field" data-field="income_account"><select name="lines-0-income_account"><option value=""></option><option value="a1">4100</option></select></div>
<div class="invoice-remove"><span hidden><input type="checkbox" name="lines-0-DELETE"></span><button type="button" data-remove-line>×</button></div>
<p data-line-help hidden></p></fieldset></div>
<template id="empty-line"></template><button type="button" id="add-line">Add line</button></form></div>
<script type="application/json" id="bill-catalog">{"plain":{"description":"","code":"Lumber","rate":"300.00","expense_account":"e1"}}</script>
<table><tbody id="bill-lines"><tr><td><select name="lines-0-item"><option value=""></option><option value="plain">Lumber</option></select></td>
<td><input name="lines-0-description" value=""></td><td><input name="lines-0-rate" value=""></td>
<td><select name="lines-0-expense_account"><option value=""></option><option value="e1">6105</option></select></td></tr></tbody></table>
<script src="${asset('sales/invoices.js')}"></script><script src="${asset('payables/bills/bills.js')}"></script></body></html>`,
  // Item 12: an invoice line whose quantity was refused, as sales/invoices/line_fields.html now renders it
  // (the error list after the line's cells), and as it rendered before (inside the 76px quantity cell).
  narrow: `<!doctype html><html><head><meta charset="utf-8">
<link rel="stylesheet" href="${asset('ledger/app.css')}"><link rel="stylesheet" href="${asset('ledger/hybrid.css')}">
<link rel="stylesheet" href="${asset('sales/editor.css')}"><link rel="stylesheet" href="${asset('sales/sales-pages.css')}"><link rel="stylesheet" href="${asset('sales/invoices.css')}"></head>
<body class="shell"><main><div class="document-editor invoice-editor"><form><div id="invoice-lines">
<fieldset class="invoice-row" id="now"><legend>Line 1</legend>
<div class="line-field" data-field="item"><select name="lines-0-item"><option>Display rack</option></select></div>
<div class="line-field" data-field="description"><textarea name="lines-0-description">Negative qty test</textarea></div>
<div class="line-field" data-field="quantity"><input name="lines-0-quantity" value="-1"></div>
<div class="line-field" data-field="rate"><input name="lines-0-rate" value="150.00"></div>
<div class="line-field" data-field="taxable"><input type="checkbox" name="lines-0-taxable"></div>
<div class="line-amount"><span>Amount</span><output>Calculated on save</output></div>
<div class="invoice-remove"><button type="button">×</button></div>
<ul class="errorlist" id="id_lines-0-quantity_error"><li>Enter a finite positive value with at most 4 decimal places within the supported limit.</li></ul>
<p class="invoice-line-help" hidden></p></fieldset>
<fieldset class="invoice-row" id="before"><legend>Line 2</legend>
<div class="line-field" data-field="item"><select name="lines-1-item"><option>Display rack</option></select></div>
<div class="line-field" data-field="description"><textarea name="lines-1-description">Negative qty test</textarea></div>
<div class="line-field" data-field="quantity"><input name="lines-1-quantity" value="-1"><ul class="errorlist"><li>Enter a finite positive value with at most 4 decimal places within the supported limit.</li></ul></div>
<div class="line-field" data-field="rate"><input name="lines-1-rate" value="150.00"></div>
<div class="line-field" data-field="taxable"><input type="checkbox" name="lines-1-taxable"></div>
<div class="line-amount"><span>Amount</span><output>Calculated on save</output></div>
<div class="invoice-remove"><button type="button">×</button></div></fieldset>
</div></form></div><form id="transfer-entry"><input id="id_amount" name="amount" aria-invalid="true" aria-describedby="amount-help id_amount_error"><span id="amount-help">Amount</span><ul id="id_amount_error"><li>This field is required.</li></ul></form></main><script src="${asset('ledger/shell.js')}"></script></body></html>`,
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
  const load = async (name) => {
    await win.loadFile(pages[name]);
    if (!cdp.isAttached()) {
      cdp.attach('1.3');
      await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
    }
    await until('document.readyState === "complete"');
  };
  // Real input through the DevTools protocol (a hidden headless window gets no OS input events), at a
  // desktop-sized viewport.
  const cdp = win.webContents.debugger;
  const send = (method, params) => cdp.sendCommand(method, params);
  const click = async (selector) => {
    const r = await js(\`(() => { const b = document.querySelector(\${JSON.stringify(selector)}).getBoundingClientRect(); return { x: Math.round(b.left + b.width - 6), y: Math.round(b.top + b.height / 2) }; })()\`);
    await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: r.x, y: r.y, button: 'left', buttons: 1, clickCount: 1 });
    await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: r.x, y: r.y, button: 'left', buttons: 0, clickCount: 1 });
    await sleep(80);
  };
  const type = async (text) => {
    for (const ch of text) {
      await send('Input.dispatchKeyEvent', { type: 'keyDown', text: ch, key: ch, unmodifiedText: ch });
      await send('Input.dispatchKeyEvent', { type: 'keyUp', key: ch });
    }
    await sleep(80);
  };
  const result = { commands };
  try {
    win.webContents.focus();
    // Item 1.
    await load('gate');
    await send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });
    const createVisible = () => js("(() => { const r=document.querySelector('[data-gate=setup] button').getBoundingClientRect(); return r.top >= 0 && r.bottom <= innerHeight && scrollY === 0; })()");
    result.createVisible = await createVisible();
    result.gateBefore = await js("({ count: document.getElementById('setup-step-count').textContent, step3: getComputedStyle(document.getElementById('recovery-step')).display })");
    await js("document.getElementById('id_protect').click(); true");
    result.protectedVisible = await createVisible();
    result.warningHidden = await js("getComputedStyle(document.getElementById('built-in-warning')).display === 'none'");
    result.gateProtected = await js("({ count: document.getElementById('setup-step-count').textContent, step3: getComputedStyle(document.getElementById('recovery-step')).display, fields: document.getElementById('own-password-fields').hidden })");
    await js("document.getElementById('id_protect').click(); true");
    result.gateAgain = await js("document.getElementById('setup-step-count').textContent");

    await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
    // Item 11.
    await load('dated');
    await js("sessionStorage.clear(); true");
    const now = new Date();
    const iso = (d) => d.toISOString().slice(0, 10);
    const far = iso(new Date(Date.UTC(now.getFullYear() + 1, now.getMonth(), now.getDate() + 2)));
    const past = iso(new Date(Date.UTC(now.getFullYear() - 1, now.getMonth(), now.getDate() - 2)));
    const near = iso(new Date(Date.UTC(now.getFullYear(), now.getMonth(), now.getDate() + 20)));
    result.far = far;
    await js(\`document.getElementById('id_date').value = '\${near}'; document.getElementById('save').click(); true\`);
    await until('true');
    await sleep(150);
    result.nearSent = commands.length;
    await load('dated');
    await js(\`document.getElementById('id_date').value = '\${far}'; document.getElementById('save').click(); true\`);
    await sleep(150);
    result.farFirst = { sent: commands.length, warning: await js("document.querySelector('.far-date-warning')?.textContent || ''"), role: await js("document.querySelector('.far-date-warning')?.getAttribute('role') || ''"), focused: await js("document.activeElement.id") };
    await js("document.getElementById('save').click(); true");
    await sleep(150);
    result.farSecond = commands.length;
    // Review N3: another new document (its own request key) on the same far date is warned again; the
    // same document's page shown again is not.
    const farOn = async (requestKey) => {
      await load('dated');
      await js(\`document.querySelector('[name=request_key]').value = '\${requestKey}'; document.getElementById('id_date').value = '\${far}'; document.getElementById('save').click(); true\`);
      await sleep(150);
      return { sent: commands.length, warned: await js("!!document.querySelector('.far-date-warning')") };
    };
    result.farNext = await farOn('k2');
    result.farNextAgain = await farOn('k2');
    await load('dated');
    await js(\`document.getElementById('id_date').value = '\${past}'; document.getElementById('save').click(); true\`);
    await sleep(150);
    result.pastFirst = { sent: commands.length, warned: await js("!!document.querySelector('.far-date-warning')") };
    await js(\`const d = document.getElementById('id_date'); d.value = '\${near}'; d.dispatchEvent(new Event('change', { bubbles: true })); true\`);
    result.pastCorrected = { warning: await js("!!document.querySelector('.far-date-warning')") };
    await js("document.getElementById('save').click(); true");
    await sleep(150);
    result.pastCorrected.sent = commands.length;

    // Review S1: the earlier-month notice on a void form.
    const notice = () => js("(() => { const n = document.getElementById('id_date_month_notice'); return n && { text: n.textContent, hidden: n.hidden, shown: n.offsetHeight > 0, role: n.getAttribute('role'), described: document.getElementById('id_date').getAttribute('aria-describedby'), after: n.previousElementSibling === document.getElementById('id_date').closest('p') }; })()");
    const setDate = (value, type) => js(\`(() => { const d = document.getElementById('id_date'); d.value = '\${value}'; d.dispatchEvent(new Event('\${type}', { bubbles: true })); return true; })()\`);
    await load('voidedNow');
    result.monthNow = await notice();
    await load('voided');
    result.monthLoaded = await notice();
    await setDate(process.env.FB_TEST_THIS_MONTH, 'input');
    result.monthThisMonth = await notice();
    await setDate(process.env.FB_TEST_OTHER_EARLIER, 'change');
    result.monthChanged = await notice();
    const sentBefore = commands.length;
    await js("document.getElementById('void').click(); true");
    await sleep(150);
    result.monthSaved = { sent: commands.length - sentBefore, date: commands.at(-1).fields.find(([k]) => k === 'date')[1], path: commands.at(-1).path };

    // Item 18.
    await load('numbers');
    for (const id of ['qty', 'rate', 'amount', 'memo']) {
      await click('#' + id);
      result['selected_' + id] = await js(\`(() => { const f = document.getElementById('\${id}'); return [f.selectionStart, f.selectionEnd, f.value.length]; })()\`);
      result['active_' + id] = await js('document.activeElement && document.activeElement.id');
    }
    await click('#rate');
    await type('40');
    await click('#memo');
    await type('X');
    result.typed = await js("({ rate: document.getElementById('rate').value, memo: document.getElementById('memo').value })");
    // A second click inside the focused field places the caret instead of selecting again.
    await click('#qty');
    await click('#qty');
    result.secondClick = await js("(() => { const f = document.getElementById('qty'); return f.selectionStart === f.selectionEnd; })()");
    // Focus from the keyboard (no pointer press) selects too; a later click inside places the caret.
    await js("document.getElementById('memo').focus(); document.getElementById('amount').focus(); true");
    result.keyboardFocus = await js("(() => { const f = document.getElementById('amount'); return [f.selectionStart, f.selectionEnd]; })()");
    await click('#amount');
    result.clickAfterKeyboard = await js("(() => { const f = document.getElementById('amount'); return f.selectionStart === f.selectionEnd; })()");

    // Item 6.
    await load('lines');
    const pick = async (selectName, value, scope) => js(\`(() => { const s = document.querySelector('\${scope} select[name=\${selectName}]'); s.value = '\${value}'; s.dispatchEvent(new Event('change', { bubbles: true })); return true; })()\`);
    await pick('lines-0-item', 'plain', '#invoice-lines');
    const val = (sel) => js(\`document.querySelector(\${JSON.stringify(sel)}).value\`);
    result.salesPlain = { description: await val('#invoice-lines [name=lines-0-description]'), rate: await val('#invoice-lines [name=lines-0-rate]'), quantity: await val('#invoice-lines [name=lines-0-quantity]') };
    await pick('lines-0-item', 'described', '#invoice-lines');
    result.salesDescribed = await val('#invoice-lines [name=lines-0-description]');
    await pick('lines-0-item', 'plain', '#bill-lines');
    result.bill = { description: await val('#bill-lines [name=lines-0-description]'), rate: await val('#bill-lines [name=lines-0-rate]') };

    await load('journal');
    result.journal = await js("(() => { const form=document.querySelector('form'); form.elements['lines-0-description'].value='Original'; form.elements['lines-0-debit'].value='99999999999999.995'; document.getElementById('add-line').click(); const focus=document.activeElement; focus.value='check'; focus.dispatchEvent(new Event('input',{bubbles:true})); const initial=form.elements['lines-1-account'].value; focus.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true})); return [focus.className,initial,form.elements['lines-1-account'].value,form.elements['lines-1-description'].value,document.getElementById('total-debit').textContent]; })()");
    await load('refund');
    result.refund = await js("(() => { const {bank,method,check_number:n}=document.querySelector('form').elements; const change=(el,v)=>{el.value=v;el.dispatchEvent(new Event('change'));}; change(bank,'b'); const suggested=n.value; n.value='92'; n.dispatchEvent(new Event('input')); change(method,'CASH'); const cash=new FormData(document.querySelector('form')).has('check_number'); change(method,'CHECK'); return [suggested,cash,n.value]; })()");
    // Item 12.
    await load('narrow');
    const box = (sel) => js(\`(() => { const r = document.querySelector('\${sel}').getBoundingClientRect(); return { width: Math.round(r.width), height: Math.round(r.height) }; })()\`);
    result.narrowNow = await box('#now > .errorlist');
    result.narrowBefore = await box('#before .errorlist');
    result.rowWidth = (await box('#now')).width;
    result.removed = await js("(() => { const row=document.getElementById('now'); row.classList.add('is-removed'); const rect = sel => row.querySelector(sel).getBoundingClientRect(); return [rect('.errorlist').width,rect('[data-field=rate]').width,rect('.line-amount').right <= rect('.invoice-remove').left]; })()");
    result.transfer = await js("(() => { const input=document.getElementById('id_amount'); input.value='100.00'; input.dispatchEvent(new Event('input',{bubbles:true})); return [!!document.getElementById('id_amount_error'),input.hasAttribute('aria-invalid'),input.getAttribute('aria-describedby')]; })()");
    out(result);
  } catch (e) { out({ error: String(e && e.stack || e), ...result }); }
});
`;

test('worker rounds: setup steps, far dates, void month notice, select on focus, item descriptions and line errors in a real window', { timeout: 240000 }, async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-worker-fixes-'));
  // Review S1: dates relative to today (local calendar, as the app's own today), all within a year so
  // the far-date warning stays out of the way.
  const now = new Date();
  const ymd = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  const dates = { today: ymd(now), earlier: ymd(new Date(now.getFullYear(), now.getMonth() - 2, 15)), otherEarlier: ymd(new Date(now.getFullYear(), now.getMonth() - 1, 3)), thisMonth: ymd(new Date(now.getFullYear(), now.getMonth(), 1)) };
  const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
  const monthText = (iso) => `This posts in ${MONTHS[Number(iso.slice(5, 7)) - 1]} ${iso.slice(0, 4)} and changes that month's reports. Change the date to record it in a different month.`;
  try {
    const pages = {};
    for (const [name, html] of Object.entries(PAGES)) {
      pages[name] = path.join(dir, `${name}.html`);
      fs.writeFileSync(pages[name], html.replaceAll('__TODAY__', dates.today).replaceAll('__EARLIER__', dates.earlier).replaceAll('__THIS_MONTH__', dates.thisMonth));
    }
    const main = path.join(dir, 'main.cjs');
    fs.writeFileSync(main, MAIN);
    const electron = require(path.join(root, 'node_modules', 'electron'));
    const env = { ...process.env, FB_TEST_PAGES: JSON.stringify(pages), FB_TEST_PRELOAD: path.join(root, 'preload', 'preload.cjs'), ELECTRON_ENABLE_LOGGING: '0', FB_TEST_THIS_MONTH: dates.thisMonth, FB_TEST_OTHER_EARLIER: dates.otherEarlier };
    delete env.ELECTRON_RUN_AS_NODE;
    const output = await new Promise((resolve, reject) => {
      const child = spawn(electron, ['--ozone-platform=headless', `--user-data-dir=${path.join(dir, 'profile')}`, main], { env, stdio: ['ignore', 'pipe', 'pipe'] });
      let stdout = '', stderr = '';
      child.stdout.on('data', d => { stdout += d; });
      child.stderr.on('data', d => { stderr += d; });
      const timer = setTimeout(() => { child.kill('SIGKILL'); reject(new Error(`Electron timed out\n${stderr}`)); }, 200000);
      child.on('exit', () => { clearTimeout(timer); resolve({ stdout, stderr }); });
    });
    const line = output.stdout.split('\n').find(l => l.startsWith('RESULT '));
    assert.ok(line, `no result from Electron:\n${output.stdout}\n${output.stderr}`);
    const r = JSON.parse(line.slice(7));
    assert.equal(r.error, undefined, r.error);

    // Item 1: two steps until a book password is chosen, then three; and back.
    assert.ok(r.createVisible && r.protectedVisible && r.warningHidden, JSON.stringify(r));
    assert.deepEqual(r.gateBefore, { count: '2', step3: 'none' });
    assert.deepEqual(r.gateProtected, { count: '3', step3: 'flex', fields: false });
    assert.equal(r.gateAgain, '2');

    // Item 11: a date within a year saves at once; one more than a year ahead (or back) warns first,
    // beside the date, and saves on the second try; correcting the date clears the warning.
    assert.equal(r.nearSent, 1);
    assert.equal(r.farFirst.sent, 1, 'the first save of a far date sends nothing');
    assert.match(r.farFirst.warning, /is more than a year from today\. Check the date\. If it is right, save again\./);
    assert.equal(r.farFirst.role, 'alert');
    assert.equal(r.farFirst.focused, 'id_date');
    assert.equal(r.farSecond, 2, 'saving again with the same date goes ahead (a warning, not a refusal)');
    // Review N3: a second document with the same far date is warned again; its page shown again is not.
    assert.deepEqual(r.farNext, { sent: 2, warned: true });
    assert.deepEqual(r.farNextAgain, { sent: 3, warned: false });
    assert.deepEqual(r.pastFirst, { sent: 3, warned: true });
    assert.deepEqual(r.pastCorrected, { warning: false, sent: 4 });
    const saves = r.commands.map(c => c.fields.find(([k]) => k === 'date')[1]);
    assert.equal(saves[1], r.far);

    // Review S1: a void date in an earlier month than today says which month it changes, follows the
    // date as it changes, and never stops the save; a date in this month shows nothing.
    assert.equal(r.monthNow, null);
    assert.deepEqual(r.monthLoaded, { text: monthText(dates.earlier), hidden: false, shown: true, role: 'status', described: 'id_date_month_notice', after: true });
    assert.equal(r.monthThisMonth.hidden, true);
    assert.equal(r.monthThisMonth.shown, false);
    assert.deepEqual({ text: r.monthChanged.text, shown: r.monthChanged.shown }, { text: monthText(dates.otherEarlier), shown: true });
    assert.deepEqual(r.monthSaved, { sent: 1, date: dates.otherEarlier, path: '/c/00000000-0000-0000-0000-000000000001/invoices/00000000-0000-0000-0000-000000000002/void/' });

    // Item 18: a click into a prefilled quantity, rate or amount selects it; typing replaces it.
    assert.deepEqual([r.active_qty, r.selected_qty], ["qty", [0, 5, 5]], JSON.stringify(r));
    assert.deepEqual(r.selected_rate, [0, 6, 6]);
    assert.deepEqual(r.selected_amount, [0, 5, 5]);
    assert.notDeepEqual(r.selected_memo, [0, 7, 7], 'ordinary text is not selected');
    assert.deepEqual(r.typed, { rate: '40', memo: 'Paid byX' });
    assert.equal(r.secondClick, true);
    assert.deepEqual(r.keyboardFocus, [0, 5]);
    assert.equal(r.clickAfterKeyboard, true);

    // Item 6: an item saved without a description gives its name; its own description wins.
    assert.deepEqual(r.salesPlain, { description: 'Custom shelving labor', rate: '65.00', quantity: '1' });
    assert.equal(r.salesDescribed, 'Oak bench, oiled');
    assert.deepEqual(r.bill, { description: 'Lumber', rate: '300.00' });

    // Item 12: the quantity error reads across the line, not one word per row.
    assert.deepEqual(r.journal, ['picker-input', '', 'a', '', '100,000,000,000,000.00']);
    assert.deepEqual(r.refund, ['808', false, '92']);
    assert.deepEqual(r.removed, [0, 0, true]);
    assert.deepEqual(r.transfer, [false, false, 'amount-help']);
    assert.ok(r.narrowBefore.width < 100, `before: ${JSON.stringify(r.narrowBefore)}`);
    assert.ok(r.narrowNow.width > 600, `now: ${JSON.stringify(r.narrowNow)} in a ${r.rowWidth}px row`);
    assert.ok(r.narrowNow.height < r.narrowBefore.height / 4, `now: ${JSON.stringify(r.narrowNow)}, before: ${JSON.stringify(r.narrowBefore)}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
