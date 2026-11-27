'use strict';

// The fb-app integration test supplies pages rendered from the real report routes.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {app, BrowserWindow, protocol} = require('electron');

const scratch = process.argv.at(-1);
setTimeout(() => {
  process.stderr.write('Timed out waiting for the worksheet browser test\n');
  app.exit(1);
}, 15000).unref();
const fixtures = JSON.parse(fs.readFileSync(path.join(scratch, 'worksheet.json'), 'utf8'));
const profile = process.env.FB_DESKTOP_USER_DATA;
fs.mkdirSync(profile, {recursive: true});
app.setPath('userData', profile);
protocol.registerSchemesAsPrivileged([{scheme: 'fb', privileges: {standard: true, secure: true}}]);

app.whenReady().then(async () => {
  const origin = 'fb://app';
  const requests = [];
  protocol.handle('fb', request => {
    const url = new URL(request.url);
    if (url.pathname === '/static/ledger/report-worksheet.js') {
      const script = fs.readFileSync(path.join(__dirname, '../../renderer/static/ledger/report-worksheet.js'));
      return new Response(script, {headers: {'content-type': 'text/javascript'}});
    }
    const html = fixtures.pages[url.pathname];
    if (html) {
      requests.push(url.pathname + url.search);
      return new Response(html, {headers: {'content-type': 'text/html; charset=utf-8'}});
    }
    return new Response(null, {status: 404});
  });
  const win = new BrowserWindow({show: false, webPreferences: {sandbox: true, contextIsolation: true}});
  const js = script => win.webContents.executeJavaScript(script);

  await win.loadURL(origin + fixtures.mappings);
  const mappings = await js(`(() => {
    const builder = document.querySelector('[data-worksheet-add]');
    builder.closest('details').open = true;
    const scope = builder.querySelector('[data-worksheet-scope]');
    const [activity, category] = [...builder.querySelectorAll('input, select')].slice(3);
    const account = builder.querySelector('[data-worksheet-account-id]');
    const posted = builder.querySelector('[data-worksheet-posted-id]');
    const append = builder.querySelector('[data-worksheet-append]');
    account.selectedIndex = 1;
    activity.value = 'Consulting';
    append.click();
    const missingCategory = category.getAttribute('aria-invalid') === 'true' && !category.validity.valid
      && document.activeElement === category && document.getElementById('worksheet-feedback').textContent.includes('category');
    category.value = 'gross_receipts';
    category.dispatchEvent(new Event('change', {bubbles:true}));
    append.click();
    for (const scopeName of ['journal', 'line']) {
      scope.value = scopeName;
      scope.dispatchEvent(new Event('change'));
      posted.value = '11111111-1111-4111-8111-111111111111';
      append.click();
    }
    return {missingCategory, account: account.value, rows: document.querySelector('textarea').value.split('\\n')};
  })()`);
  assert.ok(mappings.account);
  assert.ok(mappings.missingCategory);
  assert.deepEqual(mappings.rows, [
    'account|' + mappings.account + '|Consulting|gross_receipts|1w',
    'journal|11111111-1111-4111-8111-111111111111|Consulting|gross_receipts|1w',
    'line|11111111-1111-4111-8111-111111111111|Consulting|gross_receipts|1w',
  ]);

  await win.loadURL(origin + fixtures.assets);
  const flags = await js(`(() => {
    const worksheet = document.querySelector('textarea');
    const builder = document.querySelector('[data-worksheet-add]');
    const fields = builder.querySelectorAll('[data-worksheet-field]');
    const button = builder.querySelector('[data-worksheet-append]');
    const rows = ${JSON.stringify(fixtures.rows)};
    const final = rows.split('\\n').at(-1).split('|');
    const remoteMarker = document.createElement('input');
    remoteMarker.type = 'hidden';
    remoteMarker.name = '__fb_remote_page';
    remoteMarker.value = '0123456789abcdef0123456789abcdef0123456789abcdef012345';
    worksheet.form.appendChild(remoteMarker);
    worksheet.value = rows.slice(0, rows.lastIndexOf('\\n'));
    final.forEach((value, i) => { fields[i].value = value; });
    fields[0].value += '~';
    let warnings = 0;
    worksheet.reportValidity = () => { warnings++; return false; };
    button.click();
    const refused = worksheet.value !== rows && warnings === 1;
    fields[0].value = final[0];
    button.click();
    const accepted = worksheet.value === rows && warnings === 1;
    worksheet.value = ${JSON.stringify(fixtures.overRows)};
    worksheet.form.requestSubmit();
    worksheet.value = rows;
    worksheet.dispatchEvent(new Event("input", {bubbles:true}));
    return {refused, accepted, submitRefused: warnings === 2, disposal: fields[10].value};
  })()`);
  assert.deepEqual(flags, {refused: true, accepted: true, submitRefused: true, disposal: '2028-02-10'});

  // Connected-host submit keeps the session marker on the navigated URL while the stripped target stays at 3,800 bytes.
  const loadedHost = new Promise(resolve => win.webContents.once('did-finish-load', resolve));
  await js('document.querySelector("textarea").form.requestSubmit()');
  await loadedHost;
  const hostReq = requests.at(-1);
  assert.equal(
    hostReq,
    fixtures.target + '&__fb_remote_page=0123456789abcdef0123456789abcdef0123456789abcdef012345',
  );

  // Local submit without the marker agrees with Rust on every encoded byte (exact 3,800-byte target).
  const loadedLocal = new Promise(resolve => win.webContents.once('did-finish-load', resolve));
  await js(`(() => {
    const textarea = document.querySelector("textarea");
    textarea.value = ${JSON.stringify(fixtures.rows)};
    textarea.form.requestSubmit();
  })()`);
  await loadedLocal;
  assert.equal(requests.at(-1), fixtures.target);
  assert.equal(requests.at(-1).length, 3800);
  app.exit(0);
}).catch(error => {
  process.stderr.write(String(error.stack || error) + '\n', () => app.exit(1));
});
