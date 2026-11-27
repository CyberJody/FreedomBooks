'use strict';
// FY2025 home-business simulation UI fixes:
// 1. Review multi-entry match: live total, client validation, and submitting multiple match_line_<pk> fields.
// 2. Pay Bills far-date confirmation: preload stopImmediatePropagation() keeps the Pay button from locking on "Paying…",
//    and confirming on the second submit sends the form cleanly.
// 3. Review & Journal focus calls use preventScroll so editing doesn't jump the viewport.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { pathToFileURL } = require('node:url');

const root = path.resolve(__dirname, '..');
const asset = (rel) => pathToFileURL(path.join(root, 'renderer', 'static', rel)).href;
const head = `<meta charset="utf-8"><link rel="stylesheet" href="${asset('ledger/app.css')}"><link rel="stylesheet" href="${asset('bankfeeds/bankfeeds.css')}">`;

const multiMatchRow = (pk) => `<tr id="review-row-${pk}" data-fragment data-review-row="${pk}" data-status="NEW" tabindex="-1" data-draft-retained="false" data-bulk-block="" data-matching-rules="">
<td><input type="checkbox" name="sel" value="${pk}"></td>
<td class="num">2026-03-20</td><td class="acct">Tax Reserve</td><td class="desc">NYS DTF SALES TAX PAYMNT</td><td class="num out">-373.74</td>
<td class="cell-pick"><select name="kind_${pk}" aria-label="Record NYS DTF as"><option value="CHECK">Check or debit</option><option value="MATCH" selected>Match existing entries</option></select></td>
<td class="cell-cat"><span class="for-kind multi-match-pick" data-kinds="MATCH" data-multi-match data-amount="373.74">
<span class="deposit-title">Posted entries totaling 373.74</span>
<label class="deposit-receipt"><input type="checkbox" name="match_line_${pk}" value="line-277" data-amount="277.49" id="m-277"> <span>STP-000001 · 2026-03-20 · 277.49</span></label>
<label class="deposit-receipt"><input type="checkbox" name="match_line_${pk}" value="line-96" data-amount="96.25" id="m-96"> <span>JE 14 · 2026-03-20 · 96.25</span></label>
<label class="deposit-receipt"><input type="checkbox" name="match_line_${pk}" value="line-50" data-amount="50.00" id="m-50"> <span>JE 15 · 2026-03-20 · 50.00</span></label>
<output class="split-left" data-multi-match-left aria-live="polite"></output>
<span class="row-error-text split-error" role="alert" data-multi-match-error hidden></span>
</span></td>
<td class="cell-detail"></td>
<td class="row-buttons"><div class="row-actions"><button type="submit" name="accept_one" value="${pk}" class="primary" data-primary id="accept-${pk}">Accept</button></div></td>
</tr>`;

const bothMatchRow = (pk) => `<tr id="review-row-${pk}" data-fragment data-review-row="${pk}" data-status="NEW" tabindex="-1" data-draft-retained="false" data-bulk-block="" data-matching-rules="">
<td><input type="checkbox" name="sel" value="${pk}" id="sel-${pk}"></td>
<td class="num">2026-03-21</td><td class="acct">Checking</td><td class="desc">EXACT OR GROUPED</td><td class="num out">-100.00</td>
<td class="cell-pick"><select name="kind_${pk}"><option value="MATCH" selected>Match existing entry</option></select></td>
<td class="cell-cat"></td>
<td class="cell-detail"><span class="for-kind" data-kinds="MATCH">
<select name="line_${pk}"><option value="line-100" selected>JE 1 · 2026-03-21</option></select>
<details class="multi-match-details" id="details-${pk}"><summary id="summary-${pk}">Or match multiple entries</summary>
<span class="deposit-pick" data-multi-match data-amount="100.00" data-has-single="1">
<label class="deposit-receipt"><input type="checkbox" name="match_line_${pk}" value="line-40" data-amount="40.00"> <span>JE 2 · 40.00</span></label>
<label class="deposit-receipt"><input type="checkbox" name="match_line_${pk}" value="line-60" data-amount="60.00"> <span>JE 3 · 60.00</span></label>
<output class="split-left" data-multi-match-left aria-live="polite"></output>
<span class="row-error-text split-error" role="alert" data-multi-match-error hidden></span>
</span></details></span></td>
<td class="row-buttons"><div class="row-actions"><button type="submit" name="accept_one" value="${pk}" class="primary" data-primary id="accept-${pk}">Accept</button></div></td>
</tr>`;

const matchedRow = (pk) => `<tr id="review-row-${pk}" data-fragment data-review-row="${pk}" data-status="MATCHED" tabindex="-1"><td></td><td class="num">2026-03-20</td><td class="acct">Tax Reserve</td><td class="desc">NYS DTF SALES TAX PAYMNT</td><td class="num out">-373.74</td><td colspan="3" class="outcome">Matched: <a href="/c/1/journals/j1/">STP-000001</a> + <a href="/c/1/journals/j2/">JE 14</a></td><td></td></tr>`;
const status = (n, text = '') => `<div id="review-status" data-fragment data-to-review="${n}"><p class="review-count"><strong>${n} to review</strong></p>${text ? `<ul class="results"><li>${text}</li></ul>` : ''}</div>`;
const reviewPage = `<!doctype html><html><head>${head}</head><body class="shell"><div class="bankfeed review-page">
<form method="post" id="bankfeed-review" action="/c/00000000-0000-0000-0000-000000000001/bank-feeds/review/"><input type="hidden" name="fb_command_token" value="t-review"><input type="hidden" name="review_layout" value="new">
${status(2)}<div id="review-live" class="sr-only" role="status" aria-live="polite"></div>
<table class="review review-new"><tbody>${multiMatchRow(7)}${bothMatchRow(8)}</tbody></table></form></div>
<script src="${asset('bankfeeds/review.js')}"></script></body></html>`;

const payBillsPage = `<!doctype html><html><head>${head}</head><body class="shell">
<script type="application/json" id="pay-data">{"next_numbers":{"b1":"1010"}}</script>
<form method="post" id="pay-bills" action="/c/00000000-0000-0000-0000-000000000001/payables/pay-bills/">
<input type="hidden" name="fb_command_token" value="t-pay">
<select id="id_bank" name="bank"><option value="b1" selected>1010 · Business Checking</option></select>
<select id="id_method" name="method"><option value="CHECK" selected>Check</option></select>
<label id="reference-label" for="id_reference">First check no.</label>
<input id="id_reference" name="reference" value="1010">
<input type="date" id="id_date" name="date" value="2024-01-15">
<table><tbody>
<tr data-bill="bill1" data-vendor="v1" data-vendor-label="Acme Supply" data-number="BILL-1" data-due="2024-01-20" data-open="120.00" data-ap="ap1" data-ap-label="2000 · Accounts Payable">
<td><input type="checkbox" name="pay" value="bill1" checked></td>
<td><input type="text" name="amount-bill1" value="120.00"></td>
</tr>
</tbody></table>
<div id="pay-proposal"></div><div id="pay-entries"></div><div id="pay-shown"></div><div id="pay-total"></div>
<button type="submit" id="pay-submit" class="primary">Pay selected bills</button>
</form>
<script src="${asset('ledger/entries.js')}"></script>
<script src="${asset('payables/pay-bills.js')}"></script>
</body></html>`;

const fragment = (html) => ({ id: /id="([^"]+)"/.exec(html)[1], html });
const REPLIES = [
  { fragments: [status(0, '2026-03-20 NYS DTF SALES TAX PAYMNT -373.74: matched to STP-000001 + JE 14'), matchedRow(7)].map(fragment), next_token: 't-review-2' },
];

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
  const win = new BrowserWindow({ show: false, width: 1280, height: 800, webPreferences: { preload, sandbox: true, contextIsolation: true, nodeIntegration: false, backgroundThrottling: false } });
  const js = (code) => win.webContents.executeJavaScript(code);
  const until = async (code) => { for (let i = 0; i < 400; i++) { if (await js(code)) return; await sleep(25); } throw new Error('timed out: ' + code); };

  // 1. Multi-entry match in Review + keyboard disclosure test on row 8 ("Or match multiple entries").
  await win.loadFile(pages.review);
  await until('document.querySelector("[data-multi-match-left]") && document.querySelector("[data-multi-match-left]").textContent.length > 0');
  const cdp = win.webContents.debugger;
  cdp.attach('1.3');
  await cdp.sendCommand('Emulation.setFocusEmulationEnabled', { enabled: true });
  await js('document.getElementById("summary-8").focus(); true');
  await cdp.sendCommand('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13, text: '\\r', unmodifiedText: '\\r' });
  await cdp.sendCommand('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
  await sleep(100);
  const afterSummaryEnter = await js('({ open: document.getElementById("details-8").open, sel: document.getElementById("sel-8").checked })');
  const commandsAfterSummaryEnter = commands.length;
  await cdp.sendCommand('Input.dispatchKeyEvent', { type: 'keyDown', key: ' ', code: 'Space', windowsVirtualKeyCode: 32, text: ' ', unmodifiedText: ' ' });
  await cdp.sendCommand('Input.dispatchKeyEvent', { type: 'keyUp', key: ' ', code: 'Space', windowsVirtualKeyCode: 32 });
  await sleep(100);
  const afterSummarySpace = await js('({ open: document.getElementById("details-8").open, sel: document.getElementById("sel-8").checked })');
  const commandsAfterSummarySpace = commands.length;
  // Reopen disclosure, tick 1 entry (invalid: needs at least 2), collapse disclosure, press Accept ->
  // must reopen disclosure, show error visibly, and move focus to the checkbox inside it.
  await js('document.getElementById("details-8").open = true; document.querySelector("#review-row-8 input[name=match_line_8]").click(); document.getElementById("details-8").open = false; document.getElementById("accept-8").click();');
  await sleep(100);
  const afterCollapsedError = await js('({ open: document.getElementById("details-8").open, errHidden: document.querySelector("#review-row-8 [data-multi-match-error]").hidden, errText: document.querySelector("#review-row-8 [data-multi-match-error]").textContent, activeName: document.activeElement?.name || "" })');
  const commandsAfterCollapsedError = commands.length;
  cdp.detach();

  const initialTotal = await js('document.querySelector("[data-multi-match-left]").textContent');
  // Select wrong pair (277.49 + 50.00 = 327.49 != 373.74) and try to Accept -> blocked client-side.
  await js('document.getElementById("m-277").click(); document.getElementById("m-50").click();');
  const wrongTotal = await js('document.querySelector("[data-multi-match-left]").textContent');
  await js('document.getElementById("accept-7").click();');
  await sleep(100);
  const wrongError = await js('document.querySelector("[data-multi-match-error]").textContent');
  const commandsAfterWrong = commands.length;
  // Uncheck 50.00, check 96.25 -> exact match 373.74 -> submits both match_line_7 fields.
  await js('document.getElementById("m-50").click(); document.getElementById("m-96").click();');
  const exactTotal = await js('document.querySelector("[data-multi-match-left]").textContent');
  await js('document.getElementById("accept-7").click();');
  await until('document.querySelector("#review-row-7").dataset.status === "MATCHED"');
  const matchCmd = commands[0];

  // 2. Pay Bills far-date confirmation does NOT lock on "Paying…".
  await win.loadFile(pages.payBills);
  await until('document.getElementById("pay-submit") && !document.getElementById("pay-submit").disabled');
  await js('document.getElementById("pay-submit").click();');
  await sleep(150);
  const afterFirstClick = await js('({ text: document.getElementById("pay-submit").textContent, sending: document.getElementById("pay-bills").dataset.sending || "", warning: (document.querySelector(".far-date-warning") || {}).textContent || "" })');
  const commandsAfterFirstPay = commands.length;
  // Second click confirms the far date and submits.
  await js('document.getElementById("pay-submit").click();');
  await until('true');
  await sleep(150);
  const commandsAfterSecondPay = commands.length;
  const payCmd = commands[1] || null;

  out({ afterSummaryEnter, commandsAfterSummaryEnter, afterSummarySpace, commandsAfterSummarySpace, afterCollapsedError, commandsAfterCollapsedError, initialTotal, wrongTotal, wrongError, commandsAfterWrong, exactTotal, matchCmd, afterFirstClick, commandsAfterFirstPay, commandsAfterSecondPay, payCmd });
});`;

test('FY2025 UI fixes: multi-entry match live total/validation and Pay Bills far-date confirmation', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'fb-fy2025-ui-'));
  const reviewFile = path.join(dir, 'review.html');
  const payFile = path.join(dir, 'pay-bills.html');
  const mainFile = path.join(dir, 'main.cjs');
  fs.writeFileSync(reviewFile, reviewPage);
  fs.writeFileSync(payFile, payBillsPage);
  fs.writeFileSync(mainFile, MAIN);
  const electron = require(path.join(root, 'node_modules', 'electron'));
  const env = {
    ...process.env,
    FB_TEST_PAGES: JSON.stringify({ review: reviewFile, payBills: payFile }),
    FB_TEST_PRELOAD: path.join(root, 'preload', 'preload.cjs'),
    FB_TEST_REPLIES: JSON.stringify(REPLIES),
    ELECTRON_RUN_AS_NODE: '',
  };
  delete env.ELECTRON_RUN_AS_NODE;
  return new Promise((resolve, reject) => {
    const child = spawn(electron, ['--no-sandbox', '--ozone-platform=headless', mainFile], { env, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '', stderr = '';
    child.stdout.on('data', (d) => { stdout += d; });
    child.stderr.on('data', (d) => { stderr += d; });
    child.on('exit', (code) => {
      fs.rmSync(dir, { recursive: true, force: true });
      if (code !== 0) return reject(new Error(`electron exited ${code}: ${stderr}\n${stdout}`));
      const line = stdout.split('\n').find((l) => l.startsWith('RESULT '));
      assert.ok(line, `missing RESULT line: ${stdout}\n${stderr}`);
      const r = JSON.parse(line.slice('RESULT '.length));
      assert.equal(r.afterSummaryEnter.open, true, 'Enter on multi-match summary must open details');
      assert.equal(r.afterSummaryEnter.sel, false, 'Enter on multi-match summary must not toggle bulk sel');
      assert.equal(r.commandsAfterSummaryEnter, 0, 'Enter on multi-match summary must not submit the row');
      assert.equal(r.afterSummarySpace.open, false, 'Space on multi-match summary must toggle details closed');
      assert.equal(r.afterSummarySpace.sel, false, 'Space on multi-match summary must not toggle bulk sel');
      assert.equal(r.commandsAfterSummarySpace, 0, 'Space on multi-match summary must not submit the row');
      assert.equal(r.afterCollapsedError.open, true, 'validation error must reopen a collapsed multi-match disclosure');
      assert.equal(r.afterCollapsedError.errHidden, false, 'validation error must be visible');
      assert.match(r.afterCollapsedError.errText, /at least two/i);
      assert.equal(r.afterCollapsedError.activeName, 'match_line_8', 'focus must move to the multi-match checkbox inside the reopened disclosure');
      assert.equal(r.commandsAfterCollapsedError, 0, 'invalid collapsed multi-match must not submit');
      assert.match(r.initialTotal, /373\.74/);
      assert.match(r.wrongTotal, /327\.49/);
      assert.match(r.wrongError, /327\.49/);
      assert.equal(r.commandsAfterWrong, 0, 'wrong multi-match total must not submit');
      assert.match(r.exactTotal, /373\.74/);
      assert.ok(r.matchCmd, 'exact multi-match must submit');
      const sentLines = r.matchCmd.fields.filter(([k]) => k === 'match_line_7').map(([, v]) => v);
      assert.deepEqual(sentLines, ['line-277', 'line-96']);

      assert.equal(r.afterFirstClick.text, 'Pay selected bills', 'Pay button must NOT lock on Paying… during far-date warning');
      assert.notEqual(r.afterFirstClick.sending, '1');
      assert.match(r.afterFirstClick.warning, /year|date|again/i);
      assert.equal(r.commandsAfterFirstPay, 1, 'first far-date click must not submit');
      assert.equal(r.commandsAfterSecondPay, 2, 'second click must confirm and submit');
      assert.ok(r.payCmd && r.payCmd.path.endsWith('/payables/pay-bills/'));
      resolve();
    });
  });
});

test('review.js and journal.js use preventScroll on split/fragment/add-line focus', () => {
  const reviewSrc = fs.readFileSync(require.resolve('../renderer/static/bankfeeds/review.js'), 'utf8');
  assert.match(reviewSrc, /first\.querySelector\('\[data-split-amount\]'\)\.focus\(\{\s*preventScroll:\s*true\s*\}\)/);
  assert.match(reviewSrc, /\(nextOpenRow\(row\)\s*\|\|\s*row\)\.focus\(\{\s*preventScroll:\s*true\s*\}\)/);
  const journalSrc = fs.readFileSync(require.resolve('../renderer/static/ledger/journal.js'), 'utf8');
  assert.match(journalSrc, /account\.focus\(\{\s*preventScroll:\s*true\s*\}\)/);
  assert.match(journalSrc, /row\.scrollIntoView\(\{\s*block:\s*'nearest'\s*\}\)/);
});
