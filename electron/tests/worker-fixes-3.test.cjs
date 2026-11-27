'use strict';
// Bookkeeper rounds 6 and 7 and the Neon tests of 2026-11-24 (fix list items 37-45):
// the page behaviour behind items 37, 38, 39, 42, 43 and 45, and the review fixes S2 and M5, in a real
// sandboxed Electron window with the real preload, page scripts and stylesheets. The main process side is a
// stand-in for `books.run` that records each command and answers the review page with prepared fragments
// (one held back, so an action is in flight). Hidden window, headless platform: no display is used. Clicks
// and keys are real input events (DevTools protocol).
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { pathToFileURL } = require('node:url');

const root = path.resolve(__dirname, '..');
const asset = (rel) => pathToFileURL(path.join(root, 'renderer', 'static', rel)).href;
const shellHead = `<link rel="stylesheet" href="${asset('ledger/app.css')}"><link rel="stylesheet" href="${asset('ledger/hybrid.css')}">`;
// The shell's sticky title bar as ledger/base.html draws it (Find and "+ New" in it).
const titlebar = `<header class="titlebar"><a class="brand" href="#">FreedomBooks</a><div class="titlebar-tools"><form class="find" role="search" method="get"><input id="fb-find" type="search" name="q"></form><details class="menu titlebar-new"><summary id="new-menu">+ New</summary></details></div><div class="operator">Pat Tester</div></header>`;
const filler = Array.from({ length: 70 }, (_, i) => `<p style="height:40px;margin:0">Field ${i + 1}</p>`).join('');
// What the server adds after a save: the page is first shown without it, scrolled down, saved, and the
// reply reloads the same address with it (sessionStorage stands in for the server's state).
const afterSave = (saved, before) => `<script>document.write(sessionStorage.getItem('saved') ? ${JSON.stringify(saved)} : ${JSON.stringify(before)})</script>`;
const shell = (body, { banner = false } = {}) => `<!doctype html><html><head><meta charset="utf-8">${shellHead}</head><body class="shell">${banner ? '<div class="preview preview-notice" role="note">LOCAL TEST-ONLY PREVIEW · Synthetic data only</div>' : ''}${titlebar}<div class="layout"><nav class="appnav"></nav><main id="main">${body}</main></div><footer>FreedomBooks</footer></body></html>`;

const receiveRows = (rows) => rows.map((r, i) => `<tr data-invoice="${r.id}" data-balance="${r.balance}" data-receivables="1100 · Accounts Receivable"${r.chosen ? ` data-chosen="${r.chosen}"` : ''}${r.error ? ' class="has-error"' : ''}><td class="pay"><input type="hidden" name="allocations-${i}-invoice" value="${r.id}"><input type="checkbox" name="allocations-${i}-pay" id="id_allocations-${i}-pay"${r.apply ? ' checked' : ''} aria-label="Pay ${r.number}"></td><th scope="row">${r.number}</th><td class="num">${r.balance}</td><td class="num apply"><input type="text" name="allocations-${i}-amount" id="id_allocations-${i}-amount" inputmode="decimal" maxlength="32" autocomplete="off" value="${r.apply || ''}" aria-label="Amount to apply to ${r.number}"${r.error ? ` aria-invalid="true" aria-describedby="id_allocations-${i}-error"` : ''}>${r.error ? `<div id="id_allocations-${i}-error"><ul class="errorlist"><li>${r.error}</li></ul></div>` : ''}</td></tr>`).join('');
// Receive payment as sales/receipts/form.html draws it for Bluestem Bakery's two open invoices.
const receive = ({ amount = '', rows, extra = '' }) => `<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="${asset('sales/sales-pages.css')}"></head><body><div class="receive-payment">
<form method="post" id="receive-payment" novalidate><input type="hidden" name="fb_command_token" value="t-receive">
<section class="payment-fields"><div class="field field-customer"><select name="customer" id="id_customer"><option value="">Choose…</option><option value="bluestem" selected>Bluestem Bakery (TEST)</option></select></div>
<div class="field field-amount"><label for="id_amount">Amount received:</label><input type="text" name="amount" inputmode="decimal" maxlength="32" value="${amount}" required id="id_amount">${extra || '<p class="notice amount-extra" id="amount-extra" role="status" hidden></p>'}</div>
<div class="field"><select name="destination" id="id_destination"><option value="UNDEPOSITED" selected>Undeposited funds</option><option value="BANK">Bank account</option></select></div>
<div class="field" data-bank-field hidden><select name="bank" id="id_bank"><option value="">None</option></select></div></section>
<section class="open-invoices"><p class="apply-summary" data-summary aria-live="polite"></p><input type="hidden" name="allocations-TOTAL_FORMS" id="id_allocations-TOTAL_FORMS" value="${rows.length}"><p class="apply-empty" data-empty hidden></p>
<table class="apply-table" data-table><tbody id="invoice-rows">${receiveRows(rows)}</tbody><tfoot><tr><th colspan="3">Applied</th><td class="num" data-applied>—</td></tr></tfoot></table></section>
<details class="entries-panel"><table><tbody data-entries></tbody></table></details>
<button type="submit" name="action" value="save" id="save">Save payment</button></form>
<script type="application/json" id="open-invoices">[{"invoice_id":"inv6","customer_id":"bluestem","number":"INV-000006","date":"2026-07-14","due_date":"2026-07-14","total":"435.00","outstanding":"435.00","overdue":true,"receivables":"1100 · Accounts Receivable"},{"invoice_id":"inv10","customer_id":"bluestem","number":"INV-000010","date":"2026-08-24","due_date":"2026-08-24","total":"190.00","outstanding":"190.00","overdue":true,"receivables":"1100 · Accounts Receivable"}]</script>
<script type="application/json" id="payment-accounts">{"undeposited":"1200 · Undeposited Funds","banks":{}}</script>
<script src="${asset('sales/receipts.js')}"></script></div></body></html>`;
// Review M5: what the amount box adds about money no invoice takes.
const UNAPPLIED = 'Apply it to an invoice, or correct the amount received: FreedomBooks does not keep part of a payment as unapplied credit, and the payment is not saved until the amounts applied add up to the amount received.';
const bluestem = (over = {}) => [
  { id: 'inv6', number: 'INV-000006', balance: '435.00', ...(over.inv6 || {}) },
  { id: 'inv10', number: 'INV-000010', balance: '190.00', ...(over.inv10 || {}) },
];

// Review transactions as bankfeeds/review.html and review_row.html draw three rows, and the left menu's
// Review link with its count.
const reviewRow = (pk, { status = 'NEW', retained = 'false', payee = '', kind = 'CHECK' } = {}) => status === 'NEW'
  ? `<tr id="review-row-${pk}" data-fragment data-review-row="${pk}" data-status="NEW" tabindex="-1" data-draft-retained="${retained}" data-bulk-block="" data-matching-rules=""><td><input type="checkbox" name="sel" value="${pk}" id="sel-${pk}"></td><td class="desc">ROW ${pk}</td><td class="cell-pick"><select name="kind_${pk}" id="kind-${pk}"><option value="CHECK"${kind === 'CHECK' ? ' selected' : ''}>Check or debit</option><option value="JOURNAL"${kind === 'JOURNAL' ? ' selected' : ''}>Journal entry</option></select></td><td class="cell-detail"><span class="for-kind" data-kinds="CHECK JOURNAL"><input type="text" name="payee_${pk}" id="payee-${pk}" value="${payee}"></span></td><td class="row-buttons"><div class="row-actions"><button type="submit" name="accept_one" value="${pk}" class="primary" data-primary id="accept-${pk}">Accept</button></div></td></tr>`
  : `<tr id="review-row-${pk}" data-fragment data-review-row="${pk}" data-status="${status}" tabindex="-1" data-draft-retained="false" data-bulk-block="" data-matching-rules=""><td></td><td class="desc">ROW ${pk}</td><td class="outcome" colspan="3">Accepted: CHK-0000${pk}</td><td></td></tr>`;
const reviewStatus = (n) => `<div id="review-status" data-fragment data-to-review="${n}"><p class="review-count"><strong>${n} to review</strong></p></div>`;
const review = `<!doctype html><html><head><meta charset="utf-8"></head><body class="shell">
<nav class="appnav"><a href="#review" id="nav-review" aria-current="page" class="has-badge"><span>Review transactions</span><span class="nav-badge">3</span><span class="sr-only"> to review</span></a></nav>
<div class="bankfeed"><form method="post" id="bankfeed-review" action="/c/00000000-0000-0000-0000-000000000001/bank-feeds/review/"><input type="hidden" name="fb_command_token" value="t-review">
${reviewStatus(3)}<table class="review"><tbody>${reviewRow(1)}${reviewRow(2)}${reviewRow(3)}</tbody></table></form></div>
<a href="#elsewhere" id="leave">Bank feeds</a>
<script src="${asset('bankfeeds/review.js')}"></script></body></html>`;
// The stand-in server's in-place replies, in order.
const REVIEW_REPLIES = [
  // 1: row 1 (the person typed its payee) accepted; rows 2 and 3 drawn again fresh.
  { fragments: [reviewStatus(2), reviewRow(1, { status: 'ACCEPTED' }), reviewRow(2), reviewRow(3)].map(html => ({ id: /id="([^"]+)"/.exec(html)[1], html })), next_token: 't-review-2' },
  // 2: row 3 accepted while row 2 holds a typed draft, which the server keeps.
  { fragments: [reviewStatus(1), reviewRow(3, { status: 'ACCEPTED' }), reviewRow(2, { retained: 'true', payee: 'Typed payee' })].map(html => ({ id: /id="([^"]+)"/.exec(html)[1], html })), next_token: 't-review-3' },
  // 3: row 2 accepted: nothing left to review.
  { fragments: [reviewStatus(0), reviewRow(2, { status: 'ACCEPTED' })].map(html => ({ id: /id="([^"]+)"/.exec(html)[1], html })), next_token: 't-review-4' },
  // Review S2, on a new page. 4: row 1 accepted, held back while the person types on rows 2 and 3; the reply
  // draws rows 2 and 3 from what was posted (nothing typed on them then).
  { hold: true, fragments: [reviewStatus(2), reviewRow(1, { status: 'ACCEPTED' }), reviewRow(2), reviewRow(3)].map(html => ({ id: /id="([^"]+)"/.exec(html)[1], html })), next_token: 't-review-5' },
  // 5: row 3 accepted; row 2's posted draft kept, as the server keeps one.
  { fragments: [reviewStatus(1), reviewRow(3, { status: 'ACCEPTED' }), reviewRow(2, { retained: 'true', payee: 'Acme Supply Ltd' })].map(html => ({ id: /id="([^"]+)"/.exec(html)[1], html })), next_token: 't-review-6' },
];

const payData = '<script type="application/json" id="pay-data">{"next_numbers":{"b1":"1"},"today":"2026-03-20"}</script>';
const PAGES = {
  // Item 37: Company profile saved in place (company_form.html marks the message), with and without the
  // local preview banner above the title bar; Accounting setup (documents/setup.html: a `notice`).
  company: shell(`<h1>Edit company</h1>${afterSave('<p role="status" id="message" data-reveal>Company profile saved.</p>', '')}${filler}<button id="save">Save company settings</button>`),
  companyBanner: shell(`<h1>Edit company</h1>${afterSave('<p role="status" id="message" data-reveal>Company profile saved.</p>', '')}${filler}<button id="save">Save company settings</button>`, { banner: true }),
  setup: shell(`<h1>Accounting setup</h1>${afterSave('<p class="notice" role="status" id="message" data-reveal>Accounting setup saved (revision 5).</p>', '')}${filler}<button id="save">Save</button>`),
  // Item 45: a journal entry refused for its empty Memo (journal_form.html after the fix).
  journal: shell(`<h1>Make general journal entry</h1><form method="post" class="journal-entry" novalidate><section class="form-top journal-header"><div class="field-date"><input type="date" name="date" value="2026-01-01"></div>
${afterSave('<div class="field-memo" id="message" data-reveal><label for="id_memo">Memo:</label><input type="text" name="memo" maxlength="240" required aria-invalid="true" aria-describedby="id_memo_helptext id_memo_error" id="id_memo"><span class="helptext" id="id_memo_helptext">Required.</span><ul class="errorlist" id="id_memo_error"><li>This field is required.</li></ul></div>', '<div class="field-memo" id="message"><label for="id_memo">Memo:</label><input type="text" name="memo" maxlength="240" required aria-describedby="id_memo_helptext" id="id_memo"><span class="helptext" id="id_memo_helptext">Required.</span></div>')}</section>${filler}<button type="submit" id="save">Save draft</button></form>`),
  // Item 38: Pay bills by check (payables/payments/form.html, pay-bills.js): the bank is chosen first,
  // which fills the suggested first check number.
  pay: `<!doctype html><html><head><meta charset="utf-8"><style>input,select{display:block;width:300px;height:30px;margin:10px;font:16px monospace}</style></head><body>
<form method="post" id="pay-bills" novalidate><input type="hidden" name="filter_vendor" id="keep-filter-vendor" value=""><input type="hidden" name="filter_due" id="keep-filter-due" value="">
<!-- Deliberately omit the payment date: suggested check numbers still work. -->
<select name="bank" id="id_bank"><option value="">Choose…</option><option value="b1">1010 · Test Checking</option></select><select name="method" id="id_method"><option value="CHECK" selected>Check</option><option value="EFT">Electronic transfer</option></select>
<label for="id_reference" id="reference-label">First check no.</label><input name="reference" id="id_reference" value="" maxlength="120" autocomplete="off" data-select-on-focus>
<table><tbody><tr data-bill="b" data-vendor="v1" data-vendor-label="Timber Supply Co (TEST)" data-due="2026-03-04" data-open="800.00" data-ap="ap" data-ap-label="2000" data-number="BILL-000001" class="ticked"><td><input type="checkbox" name="pay" value="b" checked></td><td><span class="due-status">Overdue 16 days</span></td><td><input name="amount-b" value="800.00"></td></tr></tbody></table>
<p id="filter-note" hidden></p><div id="pay-proposal"></div><div id="pay-entries"></div><div id="pay-shown"></div><span id="pay-total"></span><input type="checkbox" id="pay-all">
<button type="submit" id="pay-submit">Pay selected bills</button></form>${payData}
<script src="${asset('payables/entries.js')}"></script><script src="${asset('payables/pay-bills.js')}"></script></body></html>`,
  // Item 38: Write check (checks/form.html, checks.js): choosing the bank fills the next check number.
  check: `<!doctype html><html><head><meta charset="utf-8"><style>input,select{display:block;width:300px;height:30px;margin:10px;font:16px monospace}</style></head><body>
<form method="post" id="purchase-entry" novalidate><select name="bank" id="id_bank"><option value="">Choose…</option><option value="b1">1010 · Test Checking</option></select>
<input type="text" name="check_number" data-select-on-focus aria-describedby="id_check_number_helptext" id="id_check_number"><input type="checkbox" name="print_later" id="id_print_later">
<input type="radio" name="payee_kind" value="OTHER" checked><span data-payee="OTHER">Payee</span><table><tbody id="check-lines"></tbody></table></form>
<script type="application/json" id="item-catalog">{}</script><script type="application/json" id="vendor-catalog">{}</script><script type="application/json" id="next-numbers">{"b1":"1102"}</script>
<script src="${asset('checks/checks.js')}"></script></body></html>`,
  // Item 39: the customer chosen, nothing ticked (the round 7 case); the newer invoice ticked first; a
  // refused save shown again with the server's warning and a row error.
  receive: receive({ rows: bluestem() }),
  receiveServer: receive({ amount: '453.00', rows: bluestem({ inv6: { apply: '435.00', chosen: '1', error: 'More than the open balance of 435.00.' }, inv10: { apply: '18.00', chosen: '0' } }),
    extra: '<p class="notice amount-extra" id="amount-extra" role="alert" data-server><strong>Nothing was saved yet.</strong> 18.00 of the 453.00 received would go to INV-000010, which you did not choose (you chose INV-000006).</p><input type="hidden" name="extra_ok" value="453.00|inv10=18.00">' }),
  review,
  bulk: review.replace('<table class="review">', `<section id="review-bulk" hidden>
    <output id="review-count"></output><select id="bulk-rule"><option value="">Keep rule</option></select>
    <select id="bulk-kind"><option value="AUTO">Auto</option></select><select id="bulk-category"><option value="">Keep category</option></select>
    <input id="bulk-set-payee" type="checkbox"><input id="bulk-payee"><button type="button" id="bulk-apply">Apply to selected</button>
    <details id="bulk-preview"><summary id="bulk-summary"></summary><ul id="bulk-changes"></ul></details><output id="bulk-result"></output>
    </section><table class="review">`),
  customers: `<!doctype html><form method="get" data-submit-status><input name="q" value="Synthetic">
    <select name="status"><option value="active">Active</option><option value="inactive">Inactive</option></select>
    <button>Search</button></form><script src="${asset('ledger/shell.js')}"></script>`,
};

const MAIN = `'use strict';
const { app, BrowserWindow, ipcMain } = require('electron');
const pages = JSON.parse(process.env.FB_TEST_PAGES), preload = process.env.FB_TEST_PRELOAD, replies = JSON.parse(process.env.FB_TEST_REPLIES);
const commands = [];
const out = (v) => { process.stdout.write('RESULT ' + JSON.stringify(v) + '\\n'); app.exit(0); };
// A reply marked "hold" is answered only when the test lets it go: the action is then in flight.
let release = null;
ipcMain.handle('fb:books.run', async (_e, command) => {
  commands.push(command);
  if (command.path && command.path.endsWith('/bank-feeds/review/')) {
    const data = replies.shift() || { fragments: [], next_token: 'none' };
    if (data.hold) await new Promise(resolve => { release = resolve; });
    return { ok: true, value: { outcome: 'done', data } };
  }
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
    await sleep(120);
  };
  const choose = (selector, value) => js(\`(() => { const s = document.querySelector(\${JSON.stringify(selector)}); s.value = \${JSON.stringify(value)}; s.dispatchEvent(new Event('change', { bubbles: true })); return true; })()\`);
  const selection = (selector) => js(\`(() => { const f = document.querySelector(\${JSON.stringify(selector)}); return [f.value, f.selectionStart, f.selectionEnd]; })()\`);
  const result = { commands };
  try {
    win.webContents.focus();
    // Item 37 and 45: scrolled down to Save, then reloaded as the preload follows a save that stays on
    // its page. The message (or the Memo box) must be wholly visible below the title bar.
    const reveal = async (name) => {
      await load(name);
      await js("sessionStorage.clear(); true");
      await load(name);
      // A page visited before any save: the browser restores its scroll on reload.
      await js("history.scrollRestoration = 'auto'; document.getElementById('save').scrollIntoView(); sessionStorage.setItem('saved', '1'); true");
      const before = await js('window.scrollY');
      const loaded = new Promise(resolve => win.webContents.once('did-finish-load', resolve));
      await js('window.location.reload(); true');
      await loaded;
      await until('document.readyState === "complete"');
      await sleep(400);
      return { before, ...(await js(\`(() => {
        const m = document.getElementById('message').getBoundingClientRect(), bar = document.querySelector('.titlebar').getBoundingClientRect();
        const hit = document.elementFromPoint(Math.round(m.left + 20), Math.round(m.top + 4));
        return { top: Math.round(m.top), bottom: Math.round(m.bottom), bar: Math.round(bar.bottom), height: window.innerHeight,
          covered: !!hit && !!hit.closest('.titlebar'), focused: document.activeElement && document.activeElement.id };
      })()\`)) };
    };
    result.company = await reveal('company');
    result.companyBanner = await reveal('companyBanner');
    result.setup = await reveal('setup');
    result.journal = await reveal('journal');

    // Item 38: Pay bills.
    await load('pay');
    await choose('#id_bank', 'b1');
    result.payFilled = await js("document.getElementById('id_reference').value");
    await click('#id_reference');
    result.paySelected = await selection('#id_reference');
    await type('101');
    result.payTyped = await js("document.getElementById('id_reference').value");
    await choose('#id_method', 'EFT');
    result.payEftMarked = await js("document.getElementById('id_reference').hasAttribute('data-select-on-focus')");
    await click('#id_bank');
    await click('#id_reference');
    await type('X');
    result.payEftTyped = await js("document.getElementById('id_reference').value");
    // Item 38: Write check.
    await load('check');
    await choose('#id_bank', 'b1');
    result.checkFilled = await js("document.getElementById('id_check_number').value");
    await click('#id_check_number');
    result.checkSelected = await selection('#id_check_number');
    await type('102');
    result.checkTyped = await js("document.getElementById('id_check_number').value");

    // Item 39: the customer chosen, nothing ticked; 453.00 typed for the 435.00 invoice.
    const rows = () => js("[...document.querySelectorAll('#invoice-rows tr')].map(r => ({ pay: r.querySelector('[type=checkbox]').checked, apply: r.querySelector('[name$=-amount]').value, chosen: (r.querySelector('[name$=-chosen]') || {}).value }))");
    const note = () => js("(() => { const n = document.getElementById('amount-extra'); return { hidden: n.hidden, text: n.textContent, role: n.getAttribute('role') }; })()");
    await load('receive');
    await click('#id_amount');
    await type('453.00');
    result.typoRows = await rows();
    result.typoNote = await note();
    result.typoSummary = await js("document.querySelector('[data-summary]').textContent");
    result.typoDescribed = await js("document.getElementById('id_amount').getAttribute('aria-describedby')");
    await js("document.getElementById('save').click(); true");
    await until('window.__fbDone || true');
    await sleep(300);
    const sent = commands.at(-1).fields;
    result.typoSent = sent.filter(([k]) => /^allocations-\\d+-(amount|chosen)$/.test(k));
    await load('receive');
    await click('#id_amount');
    await type('453.00');
    await js("const a = document.getElementById('id_amount'); a.select(); true");
    await type('435.00');
    result.fixedRows = await rows();
    result.fixedNote = await note();
    // The newer invoice ticked first, then 200.00 typed: it goes to the ticked invoice first.
    await load('receive');
    await click('#id_allocations-1-pay');
    result.tickedAmount = await js("document.getElementById('id_amount').value");
    await click('#id_amount');
    await js("const a = document.getElementById('id_amount'); a.select(); true");
    await type('200.00');
    result.tickedRows = await rows();
    result.tickedNote = await note();
    // The refused save shown again: the server's warning stays until something changes; correcting the
    // row clears the row's old error.
    await load('receiveServer');
    result.serverNote = await note();
    result.serverError = await js("!!document.getElementById('id_allocations-0-error')");
    result.serverChosen = (await rows()).map(r => r.chosen);
    await click('#id_allocations-0-amount');
    await type('400.00');
    result.correctedError = await js("({ box: !!document.getElementById('id_allocations-0-error'), invalid: document.getElementById('id_allocations-0-amount').getAttribute('aria-invalid'), cls: document.querySelector('#invoice-rows tr').className })");
    result.correctedNote = await note();
    // Review M5: following the warning's "…or untick INV-000010" keeps the 453.00 typed; what no invoice takes
    // is said at the amount box. Typing a new amount fills the invoice the page had filled again.
    await load('receiveServer');
    await click('#id_allocations-1-pay');
    result.untickAmount = await js("document.getElementById('id_amount').value");
    result.untickRows = await rows();
    result.untickNote = await note();
    result.untickSummary = await js("document.querySelector('[data-summary]').textContent");
    await load('receiveServer');
    await click('#id_amount');
    await js("const a = document.getElementById('id_amount'); a.select(); true");
    await type('460.00');
    result.refillRows = await rows();

    // Applied edits stay in Preview even though applying the same change again would do nothing.
    await load('bulk');
    await click('#sel-1');
    await click('#sel-2');
    await click('#bulk-set-payee');
    await click('#bulk-payee');
    await type('Acme');
    await click('#bulk-apply');
    result.bulk = await js("({visible:!document.querySelector('#bulk-preview').hidden, heading:document.querySelector('#bulk-summary').textContent, changes:[...document.querySelectorAll('#bulk-changes li')].map(e=>e.textContent), payees:[1,2,3].map(i=>document.querySelector('#payee-'+i).value)})");

    // Changing Show submits the GET form with both its current status and search text.
    await load('customers');
    await js("document.addEventListener('submit', e => { e.preventDefault(); window.sentFilter = [...new FormData(e.target)]; }, {once:true}); true");
    await choose('select[name=status]', 'inactive');
    result.customerFilter = await js('window.sentFilter');

    // Items 42 and 43: Review transactions.
    await load('review');
    await js("window.__confirms = []; window.confirm = (m) => { window.__confirms.push(m); return false; }; document.addEventListener('click', e => { if (e.target.closest('#leave')) e.preventDefault(); }); window.__fragments = 0; document.getElementById('bankfeed-review').addEventListener('fb:fragments', () => { window.__fragments++; }); true");
    const badge = () => js("(() => { const l = document.getElementById('nav-review'); const b = l.querySelector('.nav-badge'); return { badge: b ? b.textContent : null, words: l.querySelectorAll('.sr-only').length, cls: l.classList.contains('has-badge') }; })()");
    const leave = async () => { await click('#leave'); return js('window.__confirms.length'); };
    result.leaveClean = await leave();
    await click('#payee-1');
    await type('Corner Hardware');
    result.leaveDraft = await leave();
    await click('#accept-1');
    await until('window.__fragments === 1');
    result.reply1Fields = commands.at(-1).fields.filter(([k]) => k === 'shown' || k === 'edited' || k === 'accept_one');
    result.reply1Badge = await badge();
    result.leaveAfter1 = await leave();
    await click('#payee-2');
    await type('Typed payee');
    await click('#accept-3');
    await until('window.__fragments === 2');
    result.reply2Fields = commands.at(-1).fields.filter(([k]) => k === 'shown' || k === 'edited');
    result.reply2Badge = await badge();
    result.leaveAfter2 = await leave();
    await click('#accept-2');
    await until('window.__fragments === 3');
    result.reply3Badge = await badge();
    result.leaveAfter3 = await leave();

    // Review S2: typing on other rows while an Accept is in flight. The reply draws rows 2 and 3 from what was
    // posted; what was typed, chosen and ticked meanwhile is kept, and the typing goes on where it was.
    await load('review');
    await js("window.__confirms = []; window.confirm = (m) => { window.__confirms.push(m); return false; }; document.addEventListener('click', e => { if (e.target.closest('#leave')) e.preventDefault(); }); window.__fragments = 0; document.getElementById('bankfeed-review').addEventListener('fb:fragments', () => { window.__fragments++; }); true");
    await click('#accept-1');
    for (let i = 0; i < 400 && !release; i++) await sleep(25);
    if (!release) throw new Error('the accept was not sent');
    await js("document.querySelectorAll('tr[data-review-row]').forEach(r => { r.dataset.before = '1'; }); true");
    await click('#payee-2');
    await type('Acme Supply');
    await choose('#kind-3', 'JOURNAL');
    await click('#sel-3');
    await click('#payee-2');
    await js("const p = document.getElementById('payee-2'); p.setSelectionRange(p.value.length, p.value.length); true");
    const letGo = release; release = null; letGo();
    await until('window.__fragments === 1');
    await type(' Ltd');
    result.inflight = await js(\`(() => {
      const row = (pk) => document.getElementById('review-row-' + pk);
      return { redrawn: !row(2).dataset.before && !row(3).dataset.before, status1: row(1).dataset.status,
        payee2: document.getElementById('payee-2').value, focused: document.activeElement && document.activeElement.id,
        retained2: row(2).dataset.draftRetained, kind3: document.getElementById('kind-3').value, retained3: row(3).dataset.draftRetained,
        sel3: document.getElementById('sel-3').checked, sel2: document.getElementById('sel-2').checked };
    })()\`);
    result.inflightLeave = await leave();
    await click('#accept-3');
    await until('window.__fragments === 2');
    result.inflightSent = commands.at(-1).fields.filter(([k, v]) => ['edited', 'payee_2', 'kind_3', 'accept_one'].includes(k) || (k === 'sel'));
    result.inflightAfter = await js("({ payee2: document.getElementById('payee-2').value, retained2: document.getElementById('review-row-2').dataset.draftRetained })");
    out(result);
  } catch (e) { out({ error: String(e && e.stack || e), ...result }); }
});
`;

test('worker rounds 6-7 and Neon: saved message below the title bar, suggested numbers, an over-typed payment, review drafts and the menu count', { timeout: 300000 }, async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-worker-fixes-3-'));
  try {
    const pages = {};
    for (const [name, html] of Object.entries(PAGES)) {
      pages[name] = path.join(dir, `${name}.html`);
      fs.writeFileSync(pages[name], html);
    }
    const main = path.join(dir, 'main.cjs');
    fs.writeFileSync(main, MAIN);
    const electron = require(path.join(root, 'node_modules', 'electron'));
    const env = { ...process.env, FB_TEST_PAGES: JSON.stringify(pages), FB_TEST_PRELOAD: path.join(root, 'preload', 'preload.cjs'), FB_TEST_REPLIES: JSON.stringify(REVIEW_REPLIES), ELECTRON_ENABLE_LOGGING: '0' };
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

    // Item 37: after the reload the message is wholly in the window and below the title bar, not under it
    // (the round 6 screens: at the window's top edge, under the 48 px bar). The same for the journal's Memo
    // (item 45), which also gets the focus.
    for (const name of ['company', 'companyBanner', 'setup', 'journal']) {
      const v = r[name];
      assert.ok(v.before > 0, `${name}: ${JSON.stringify(v)}`);
      assert.ok(v.bar > 0 && v.top >= v.bar && v.bottom <= v.height && !v.covered, `${name}: ${JSON.stringify(v)}`);
    }
    assert.equal(r.journal.focused, 'id_memo');

    // Item 38: the suggested number is selected when the box is clicked, so typing replaces it.
    assert.equal(r.payFilled, '1');
    assert.deepEqual(r.paySelected, ['1', 0, 1]);
    assert.equal(r.payTyped, '101', 'was 1101');
    assert.equal(r.payEftMarked, false, 'a free-text reference is not marked');
    assert.equal(r.payEftTyped, '101X', 'and is edited as usual');
    assert.equal(r.checkFilled, '1102');
    assert.deepEqual(r.checkSelected, ['1102', 0, 4]);
    assert.equal(r.checkTyped, '102', 'was 1102102');

    // Item 39: 453.00 typed: 435.00 on the oldest invoice, which counts as chosen; the 18.00 on the other
    // one is said at the amount box, and the rows say which invoice was chosen.
    assert.deepEqual(r.typoRows, [{ pay: true, apply: '435.00', chosen: '1' }, { pay: true, apply: '18.00', chosen: '0' }]);
    assert.deepEqual(r.typoNote, { hidden: false, role: 'status', text: '18.00 of the 453.00 would go to INV-000010, which you did not choose. Saving asks you to confirm this.' });
    assert.match(r.typoSummary, /Received 453\.00/);
    assert.match(r.typoDescribed, /amount-extra/);
    assert.deepEqual(Object.fromEntries(r.typoSent), { 'allocations-0-amount': '435.00', 'allocations-0-chosen': '1', 'allocations-1-amount': '18.00', 'allocations-1-chosen': '0' });
    // Corrected to 435.00: nothing on the other invoice, no note.
    assert.deepEqual(r.fixedRows, [{ pay: true, apply: '435.00', chosen: '1' }, { pay: false, apply: '', chosen: '0' }]);
    assert.equal(r.fixedNote.hidden, true);
    // The newer invoice ticked first: 200.00 fills it first; the 10.00 left goes to the older one, said.
    assert.equal(r.tickedAmount, '190.00');
    assert.deepEqual(r.tickedRows, [{ pay: true, apply: '10.00', chosen: '0' }, { pay: true, apply: '190.00', chosen: '1' }]);
    assert.equal(r.tickedNote.text, '10.00 of the 200.00 would go to INV-000006, which you did not choose. Saving asks you to confirm this.');
    // Shown again after the refused save: the server's words stay, the marks are kept.
    assert.deepEqual({ hidden: r.serverNote.hidden, role: r.serverNote.role }, { hidden: false, role: 'alert' });
    assert.match(r.serverNote.text, /^Nothing was saved yet\. 18\.00 of the 453\.00 received/);
    assert.equal(r.serverError, true);
    assert.deepEqual(r.serverChosen, ['1', '0']);
    // The row corrected: its old red error is gone; the note follows the new amounts. Review M5: the amount
    // received stays as typed (453.00), so the note also says what is not applied.
    assert.deepEqual(r.correctedError, { box: false, invalid: null, cls: '' });
    assert.equal(r.correctedNote.hidden, false);
    assert.equal(r.correctedNote.text, `18.00 of the 453.00 would go to INV-000010, which you did not choose. Saving asks you to confirm this. 35.00 of the 453.00 received is not applied to an invoice. ${UNAPPLIED}`);
    // Review M5: unticking INV-000010 after the warning keeps the 453.00 typed (it was lowered to 435.00); the
    // 18.00 is said at the amount box, and Save is refused by the server until it is applied.
    assert.equal(r.untickAmount, '453.00', 'was 435.00');
    assert.deepEqual(r.untickRows, [{ pay: true, apply: '435.00', chosen: '1' }, { pay: false, apply: '', chosen: '0' }]);
    assert.deepEqual(r.untickNote, { hidden: false, role: 'status', text: `18.00 of the 453.00 received is not applied to an invoice. ${UNAPPLIED}` });
    assert.equal(r.untickSummary, 'Received 453.00 · 18.00 still to apply');
    // A row the page filled is not one the person unticked: a new amount fills it again.
    assert.deepEqual(r.refillRows, [{ pay: true, apply: '435.00', chosen: '1' }, { pay: true, apply: '25.00', chosen: '0' }]);

    // Item 42: "unsaved review drafts" only while a typed draft remains. Item 43: the menu count follows.
    assert.deepEqual(r.bulk, {visible:true, heading:'Applied changes', changes:['ROW 1: Payee: (blank) → Acme','ROW 2: Payee: (blank) → Acme'], payees:['Acme','Acme','']});
    assert.deepEqual(r.customerFilter, [['q','Synthetic'], ['status','inactive']]);
    assert.equal(r.leaveClean, 0);
    assert.equal(r.leaveDraft, 1, 'a typed payee is a draft');
    assert.deepEqual(Object.fromEntries(r.reply1Fields), { shown: '1,2,3', edited: '1', accept_one: '1' });
    assert.deepEqual(r.reply1Badge, { badge: '2', words: 1, cls: true });
    assert.equal(r.leaveAfter1, 1, 'the accepted row left no draft: no new question');
    assert.deepEqual(Object.fromEntries(r.reply2Fields), { shown: '1,2,3', edited: '2' });
    assert.deepEqual(r.reply2Badge, { badge: '1', words: 1, cls: true });
    assert.equal(r.leaveAfter2, 2, 'row 2 still holds its typed draft');
    assert.deepEqual(r.reply3Badge, { badge: null, words: 0, cls: false });
    assert.equal(r.leaveAfter3, 2, 'nothing typed is left');

    // Review S2: rows 2 and 3 were drawn again by the reply; the payee typed on row 2, the kind chosen and the
    // tick on row 3 while the accept was in flight are on the new rows, the typing went on in the same box,
    // and both rows count as drafts.
    assert.deepEqual(r.inflight, { redrawn: true, status1: 'ACCEPTED', payee2: 'Acme Supply Ltd', focused: 'payee-2', retained2: 'true', kind3: 'JOURNAL', retained3: 'true', sel3: true, sel2: false });
    assert.equal(r.inflightLeave, 1, 'the drafts warning knows of them');
    assert.deepEqual(Object.fromEntries(r.inflightSent.filter(([k]) => k !== 'sel')), { kind_3: 'JOURNAL', payee_2: 'Acme Supply Ltd', accept_one: '3', edited: '2,3' });
    assert.deepEqual(r.inflightSent.filter(([k]) => k === 'sel'), [['sel', '3']]);
    assert.deepEqual(r.inflightAfter, { payee2: 'Acme Supply Ltd', retained2: 'true' });
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
