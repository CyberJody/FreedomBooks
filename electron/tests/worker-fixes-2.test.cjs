'use strict';
// Bookkeeper test rounds 4 and 5 of 2026-11-23: the page behaviour behind items
// 25 and 30, in a real sandboxed Electron window with the real preload and page scripts. Hidden window,
// headless platform: no display is used.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { pathToFileURL } = require('node:url');

const root = path.resolve(__dirname, '..');
const asset = (rel) => pathToFileURL(path.join(root, 'renderer', 'static', rel)).href;
const filler = Array.from({ length: 60 }, (_, i) => `<p style="height:40px;margin:0">Field ${i + 1}</p>`).join('');

const PAGES = {
  // Item 30: a page saved in place (Company profile) shows its message at the top and its Save button at
  // the bottom; with the message marked data-reveal, and the same page without the mark.
  saved: `<!doctype html><html><head><meta charset="utf-8"></head><body><h1>Edit company</h1>
<p role="status" id="message" data-reveal>Company profile saved.</p>${filler}<button id="save">Save</button></body></html>`,
  unmarked: `<!doctype html><html><head><meta charset="utf-8"></head><body><h1>Edit company</h1>
<p role="status" id="message">Company profile saved.</p>${filler}<button id="save">Save</button></body></html>`,
  // Item 25: Pay bills as payables/payments/form.html prints it after a refused run sent with "Due on or
  // before 2026-05-31": the June bill stays out of sight; the May bill (ticked, with its problem) in sight.
  pay: `<!doctype html><html><head><meta charset="utf-8"></head><body>
<form method="get" class="pay-filter" id="pay-filter"><select name="vendor" id="filter-vendor"><option value="">All vendors</option><option value="v1">Timber</option><option value="v2">Valley</option></select>
<input type="date" name="due" id="filter-due" value="2026-05-31"><button type="submit" id="filter-apply">Show</button></form>
<form method="post" id="pay-bills" novalidate><input type="hidden" name="filter_vendor" id="keep-filter-vendor" value=""><input type="hidden" name="filter_due" id="keep-filter-due" value="2026-05-31">
<input type="date" name="date" id="id_date" value="2026-05-28" required>
<select name="bank" id="id_bank"><option value="b1" selected>1000</option></select><select name="method" id="id_method"><option value="CHECK" selected>Check</option></select>
<span id="reference-label">First check no.</span><input name="reference" id="id_reference" value="130">
<table><tbody>
<tr data-bill="may" data-vendor="v1" data-vendor-label="Timber" data-due="2026-05-25" data-open="420.00" data-ap="ap" data-ap-label="2000" data-number="BILL-1" class="ticked"><td><input type="checkbox" name="pay" value="may" checked></td><td><span class="due-status">Overdue 3 days</span></td><td><input name="amount-may" value="500.00"><ul class="errorlist row-error"><li>This is more than the open balance of 420.00.</li></ul></td></tr>
<tr data-bill="late" data-vendor="v1" data-vendor-label="Timber" data-due="2026-06-15" data-open="10.00" data-ap="ap" data-ap-label="2000" data-number="BILL-3" class="ticked"><td><input type="checkbox" name="pay" value="late" checked></td><td><span class="due-status">Due in 18 days</span></td><td><input name="amount-late" value="10.00"><ul class="errorlist row-error"><li>The payment date is before this bill's date.</li></ul></td></tr>
<tr data-bill="june" data-vendor="v2" data-vendor-label="Valley" data-due="2026-06-30" data-open="88.20" data-ap="ap" data-ap-label="2000" data-number="BILL-2" hidden><td><input type="checkbox" name="pay" value="june"></td><td><span class="due-status">Due in 33 days</span></td><td><input name="amount-june" value=""></td></tr>
</tbody></table><p id="filter-note" hidden></p><div id="pay-proposal"></div><div id="pay-entries"></div><div id="pay-shown"></div><span id="pay-total"></span><input type="checkbox" id="pay-all">
<button type="submit" id="pay-submit">Pay selected bills</button></form>
<script type="application/json" id="pay-data">{"next_numbers":{"b1":"130"},"today":"2026-05-28"}</script>
<script src="${asset('payables/entries.js')}"></script><script src="${asset('payables/pay-bills.js')}"></script></body></html>`,
};

const MAIN = `'use strict';
const { app, BrowserWindow } = require('electron');
const pages = JSON.parse(process.env.FB_TEST_PAGES), preload = process.env.FB_TEST_PRELOAD;
const out = (v) => { process.stdout.write('RESULT ' + JSON.stringify(v) + '\\n'); app.exit(0); };
const sleep = (ms) => new Promise(r => setTimeout(r, ms));
app.whenReady().then(async () => {
  const win = new BrowserWindow({ show: false, width: 1200, height: 700, webPreferences: { preload, sandbox: true, contextIsolation: true, nodeIntegration: false, backgroundThrottling: false } });
  const js = (code) => win.webContents.executeJavaScript(code);
  const until = async (code) => { for (let i = 0; i < 400; i++) { if (await js(code)) return; await sleep(25); } throw new Error('timed out: ' + code); };
  const cdp = win.webContents.debugger;
  const load = async (name) => {
    await win.loadFile(pages[name]);
    if (!cdp.isAttached()) {
      cdp.attach('1.3');
      await cdp.sendCommand('Emulation.setDeviceMetricsOverride', { width: 1200, height: 700, deviceScaleFactor: 1, mobile: false });
    }
    await until('document.readyState === "complete"');
  };
  // Scrolled down to Save, then reloaded the way the preload follows a save that stays on its page.
  const savedAndReloaded = async (name) => {
    await load(name);
    await js("document.getElementById('save').scrollIntoView(); true");
    const before = await js('window.scrollY');
    const loaded = new Promise(resolve => win.webContents.once('did-finish-load', resolve));
    await js('window.location.reload(); true');
    await loaded;
    await until('document.readyState === "complete"');
    await sleep(400);
    return { before, after: await js('window.scrollY'), inView: await js("(() => { const r = document.getElementById('message').getBoundingClientRect(); return r.top >= 0 && r.bottom <= window.innerHeight; })()") };
  };
  const result = {};
  try {
    // Item 30.
    result.unmarked = await savedAndReloaded('unmarked');
    result.saved = await savedAndReloaded('saved');
    // Item 25.
    await load('pay');
    const state = () => js("({ keepDue: document.getElementById('keep-filter-due').value, keepVendor: document.getElementById('keep-filter-vendor').value, hidden: [...document.querySelectorAll('tr[data-bill]')].filter(r => r.hidden).map(r => r.dataset.bill) })");
    result.payLoaded = await state();
    const changePaymentDate = (date) => js(\`(() => {
      const input = document.getElementById('id_date');
      input.value = '\${date}';
      input.dispatchEvent(new Event('input', { bubbles: true }));
      return [...document.querySelectorAll('tr[data-bill]')].map(row => [
        row.querySelector('.due-status').textContent, row.classList.contains('overdue')
      ]);
    })()\`);
    result.payBackdated = await changePaymentDate('2026-05-25');
    result.payLater = await changePaymentDate('2026-06-14');
    result.payNoDate = await changePaymentDate('');
    await js("const d = document.getElementById('filter-due'); d.value = '2026-06-30'; d.dispatchEvent(new Event('change', { bubbles: true })); true");
    result.payWider = await state();
    await js("const v = document.getElementById('filter-vendor'); v.value = 'v2'; v.dispatchEvent(new Event('change', { bubbles: true })); true");
    result.payVendor = await state();
    out(result);
  } catch (e) { out({ error: String(e && e.stack || e), ...result }); }
});
`;

test('worker rounds 4 and 5: a saved message in view after an in-place save; Pay bills carries its filters', { timeout: 240000 }, async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-worker-fixes-2-'));
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
      const timer = setTimeout(() => { child.kill('SIGKILL'); reject(new Error(`Electron timed out\n${stderr}`)); }, 200000);
      child.on('exit', () => { clearTimeout(timer); resolve({ stdout, stderr }); });
    });
    const line = output.stdout.split('\n').find(l => l.startsWith('RESULT '));
    assert.ok(line, `no result from Electron:\n${output.stdout}\n${output.stderr}`);
    const r = JSON.parse(line.slice(7));
    assert.equal(r.error, undefined, r.error);

    // Item 30: without the mark the reload keeps the scroll at Save and the message is out of view (the
    // worker's report); with it, the message is in view.
    assert.ok(r.unmarked.before > 0 && r.unmarked.after > 0 && !r.unmarked.inView, JSON.stringify(r.unmarked));
    assert.ok(r.saved.before > 0, JSON.stringify(r.saved));
    assert.equal(r.saved.inView, true, JSON.stringify(r.saved));

    // Item 25: the pay form carries the filters as they change; a row with a problem stays in sight.
    assert.deepEqual(r.payLoaded, { keepDue: '2026-05-31', keepVendor: '', hidden: ['june'] });
    assert.deepEqual(r.payBackdated, [['Due on payment date', false], ['Due in 21 days', false], ['Due in 36 days', false]]);
    assert.deepEqual(r.payLater, [['Overdue 20 days', true], ['Due in 1 day', false], ['Due in 16 days', false]]);
    assert.deepEqual(r.payNoDate, Array(3).fill(['Choose a payment date', false]));
    assert.deepEqual(r.payWider, { keepDue: '2026-06-30', keepVendor: '', hidden: [] });
    assert.deepEqual(r.payVendor, { keepDue: '2026-06-30', keepVendor: 'v2', hidden: [] }, 'the Timber bills have problems, so they stay in sight');
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
