'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { pathToFileURL } = require('node:url');

const root = path.resolve(__dirname, '..');
const reviewScript = pathToFileURL(path.join(root, 'renderer/static/bankfeeds/review.js')).href;
const company = '00000000-0000-7000-8000-000000000001';
const reviewRow = (id, matches) => `
<tr id="review-row-${id}" data-review-row="${id}" data-status="NEW" data-matching-rules="${matches}" data-bulk-block="">
<td><input name="sel" type="checkbox" value="${id}"></td><td class="desc">Office supply</td>
<td><select name="kind_${id}"><option value="JOURNAL">Journal</option><option value="TRANSFER">Transfer</option></select></td>
<td><select name="account_${id}"><option value="">Choose</option><option value="expense">Supplies</option></select>
<input name="payee_${id}"></td>
<td><span class="for-kind" data-kinds="TRANSFER" hidden><select name="to_${id}" data-transfer-account>
<option value="">Transfer from…</option><option value="checking">Card payment · Checking</option>
<option value="card">Balance transfer · Mastercard</option></select></span></td>
</tr>`;

const page = `<!doctype html><meta charset="utf-8"><body>
<form id="discover" method="post" action="/c/${company}/bank-feeds/site/sync/">
<input name="fb_command_token" value="sync-token"><button type="submit" name="action" value="discover" id="discover-button">Discover accounts</button>
<button type="submit">Sync now</button></form>
<form id="bankfeed-mapping" method="post" action="/c/${company}/bank-feeds/site/accounts/">
<input name="fb_command_token" value="map-token"><table><tbody><tr><td>
<select name="map_card"><option value="">Unmapped</option><option value="__new__">Add new account</option></select>
<fieldset class="new-feed-account" hidden disabled><input name="name_card" value="Visa"><input name="type_card" value="CREDIT_CARD"></fieldset>
</td><td><input type="date" name="import_from_card" value="2025-10-01">
<input type="hidden" name="import_state_card" value="PENDING">
<input type="hidden" name="import_expected_card" value=""></td></tr></tbody></table>
<button type="submit" id="map-button">Save settings</button></form>
<form method="post" id="bankfeed-review" action="/c/${company}/bank-feeds/review/">
<input name="fb_command_token" value="review-token"><section id="review-bulk" hidden>
<output id="review-count"></output><select id="bulk-rule"><option value="">Keep</option>
<option value="rule" data-account="expense" data-payee="Acme">Checking only</option></select>
<select id="bulk-kind"><option value="AUTO">Auto</option></select>
<select id="bulk-category"><option value="">Keep category</option><option value="expense">Supplies</option></select>
<input id="bulk-set-payee" type="checkbox"><input id="bulk-payee"><button type="button" id="bulk-apply">Apply</button>
<details id="bulk-preview"><summary id="bulk-summary"></summary><ul id="bulk-changes"></ul></details><output id="bulk-result"></output>
</section><table class="review"><tbody>${reviewRow(1, 'rule')}${reviewRow(2, '')}</tbody></table></form>
<script src="${reviewScript}"></script>`;

const main = `'use strict';
const { app, BrowserWindow, ipcMain } = require('electron');
const [page, preload] = process.argv.slice(-2);
const seen = [];
ipcMain.handle('fb:books.run', (_e, command) => {
  seen.push(command);
  return { ok: true, value: { outcome: 'done', data: {} } };
});
app.whenReady().then(async () => {
  const window = new BrowserWindow({ show: false, webPreferences: { preload, sandbox: true, contextIsolation: true, backgroundThrottling: false } });
  await window.loadFile(page);
  const state = await window.webContents.executeJavaScript(\`(() => {
    const choose = (name, value) => {
      const field = document.querySelector(name);
      field.value = value;
      field.dispatchEvent(new Event('change', {bubbles: true}));
      return field;
    };
    const transfer = document.querySelector('[name=to_2]');
    const form = document.querySelector('#bankfeed-review');
    const hidden = transfer.disabled && !new FormData(form).has('to_2');
    choose('[name=kind_2]', 'TRANSFER');
    choose('[name=to_2]', 'checking');
    const payment = !transfer.disabled && new FormData(form).get('to_2') === 'checking';
    choose('[name=kind_2]', 'JOURNAL');
    const dormant = transfer.disabled && transfer.value === 'checking' && !new FormData(form).has('to_2');
    for (const pick of form.querySelectorAll('[name=sel]')) pick.click();
    choose('#bulk-rule', 'rule');
    const preview = document.querySelector('#bulk-changes').textContent;
    choose('[name=map_card]', '__new__');
    document.querySelector('#discover-button').click();
    document.querySelector('#map-button').click();
    return { hidden, payment, dormant, preview };
  })()\`);
  for (let i = 0; i < 100 && seen.length < 2; i++) await new Promise(resolve => setTimeout(resolve, 20));
  process.stdout.write(JSON.stringify({ state, seen }) + '\\n');
  app.exit(0);
}).catch(error => { process.stderr.write(String(error.stack)); app.exit(1); });
`;

test('bank-feed discovery, import start and scoped transfer controls keep their form intent', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'fb-bank-policy-'));
  try {
    const html = path.join(dir, 'index.html');
    const script = path.join(dir, 'main.cjs');
    fs.writeFileSync(html, page);
    fs.writeFileSync(script, main);
    const electron = require(path.join(root, 'node_modules/electron'));
    const env = { ...process.env, FB_DESKTOP_USER_DATA: path.join(dir, 'profile') };
    delete env.ELECTRON_RUN_AS_NODE;
    const result = await new Promise((resolve, reject) => {
      const child = spawn(electron, ['--no-sandbox', '--ozone-platform=headless', script, html, path.join(root, 'preload/preload.cjs')],
        { env, stdio: ['ignore', 'pipe', 'pipe'] });
      let stdout = '', stderr = '';
      child.stdout.on('data', data => { stdout += data; });
      child.stderr.on('data', data => { stderr += data; });
      child.on('error', reject);
      child.on('exit', code => code === 0 ? resolve(JSON.parse(stdout)) : reject(new Error(`Electron exited ${code}: ${stderr}`)));
    });
    assert.equal(result.state.hidden, true);
    assert.equal(result.state.payment, true);
    assert.equal(result.state.dormant, true);
    assert.match(result.state.preview, /Does not match this rule/);
    assert.match(result.state.preview, /Category: Choose → Supplies/);
    assert.equal(result.seen.length, 2);
    const [discover, mapping] = result.seen;
    assert.equal(Object.fromEntries(discover.fields).action, 'discover');
    assert.deepEqual(Object.fromEntries(mapping.fields), {
      map_card: '__new__', name_card: 'Visa', type_card: 'CREDIT_CARD',
      import_from_card: '2025-10-01', import_state_card: 'PENDING', import_expected_card: '',
    });
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
