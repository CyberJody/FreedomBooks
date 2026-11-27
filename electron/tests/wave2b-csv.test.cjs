'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {execFile} = require('node:child_process');
const {promisify} = require('node:util');
const guards = require('../main/guards.cjs');
const remote = require('../main/remote-mode.cjs');

const reports = [
  ['customer_statements', 'customer-statements'],
  ['cash_flow', 'cash-flow'],
  ['comparative_profit_and_loss', 'comparative-profit-and-loss'],
  ['sales_by_customer_item', 'sales-by-customer-item'],
  ['schedule_c_activity', 'schedule-c', ['mappings', 'account|synthetic|Tech consulting|gross_receipts|1w']],
  ['asset_depreciation_schedule', 'asset-depreciation', ['assets', 'Equipment||||2028-02-01|20000|0|straight_line|24|0']],
];
for (const [name, slug] of reports) {
  test(`${slug} CSV has attachment headers and preserves the UTF-8 BOM on local and connected hosts`, async () => {
    const route = `reports:${name}`;
    const path = `fb://app/c/11111111-1111-4111-8111-111111111111/reports/${slug}/`;
    const url = new URL(`${path}?output=csv`);
    const repeated = new URL(`${path}?output=print&output=csv`);
    const body = "\uFEFFReport,Example\r\nCustomer,'=<script>example</script>\r\n";
    assert.equal(guards.csvExportFilename(route, url, 200, body), 'report.csv');
    assert.equal(remote.csvPage(route, url, 200, body), true);
    assert.equal(remote.csvPage(route, repeated, 200, body), true);
    assert.equal(guards.csvExportResponse(route, repeated, 200, body).headers.get('content-type'), 'text/csv; charset=utf-8');
    for (const [override, filename] of [[undefined, 'report.csv'], ['export.csv', 'export.csv']]) {
      const response = guards.csvExportResponse(route, url, 200, body, override);
      assert.equal(response.status, 200);
      assert.equal(response.headers.get('content-type'), 'text/csv; charset=utf-8');
      assert.equal(response.headers.get('content-disposition'), `attachment; filename="${filename}"`);
      assert.equal(response.headers.get('x-content-type-options'), 'nosniff');
      assert.equal(response.headers.get('content-security-policy'), guards.CSP);
      const bytes = new Uint8Array(await response.arrayBuffer());
      assert.deepEqual(bytes.subarray(0, 3), Uint8Array.of(0xef, 0xbb, 0xbf));
      assert.equal(new TextDecoder().decode(bytes), body.slice(1));
    }

    const unmarked = '<html><script>bad response</script>';
    assert.equal(remote.csvPage(route, url, 200, unmarked), true);
    const refused = guards.csvExportResponse(route, url, 200, unmarked);
    assert.equal(refused.status, 502);
    assert.equal(refused.headers.get('content-type'), 'text/plain; charset=utf-8');
    assert.equal(await refused.text(), 'Report export unavailable.');
    assert.equal(guards.csvExportResponse(route, url, 400, unmarked), null);

    const printLast = new URL(`${path}?output=csv&output=print`);
    assert.equal(remote.csvPage(route, printLast, 200, unmarked), false);
    assert.equal(guards.csvExportResponse(route, printLast, 200, unmarked), null);
  });
}

// Use the actual sandboxed preload, IPC, renderer guards and Save handlers. The
// native book and the remote transport return synthetic pages and export bytes.
// The fb-app Wave 2b tests exercise the worksheet builder against rendered report pages.
const desktop = String.raw`
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {setTimeout: delay} = require('node:timers/promises');
const {app, BrowserWindow, dialog} = require('electron');
const root = process.env.FB_TEST_ELECTRON_ROOT;
const scratch = process.env.FB_TEST_SCRATCH;
app.setPath('appData', scratch);
process.env.FB_NATIVE_MODULE = __filename;
const reports = JSON.parse(process.env.FB_TEST_REPORTS);
const company = '11111111-1111-4111-8111-111111111111';
const body = '\uFEFFReport,Example\r\n';
const commands = [], dialogs = [];
let connected = false;
let hostUser = 'User A';

function render(target) {
  const url = new URL(target, 'fb://app');
  const slug = url.pathname.split('/').at(-2);
  const report = reports.find(([, candidate]) => candidate === slug);
  const route = report && 'reports:' + report[0];
  const output = url.searchParams.getAll('output').at(-1);
  const filters = report?.[2] ? '&amp;' + new URLSearchParams([report[2]]) : '';
  let html = '<head><title>Synthetic report</title></head><body>';
  if (connected && hostUser) html += '<p id="signed-in-user">' + hostUser + '</p>';
  if (['/desktop/session/', '/desktop/sign-in/'].includes(url.pathname)) {
    const action = url.pathname === '/desktop/session/' ? 'signout' : 'signin';
    html += '<form method="post"><input name="fb_command_token" value="test"><button name="action" value="' + action + '">' + action + '</button></form>';
  }
  if (output === 'csv') return {status: 200, route, html: body};
  if (output === 'print') html += '<p id="print-view">Print view</p>';
  if (report && !output) {
    for (const [id, values] of [
      ['plain', 'output=csv'],
      ['csv-last', 'output=print&output=csv'],
      ['print-last', 'output=csv&output=print'],
    ]) {
      html += '<a id="' + id + '" href="?start=2028-02-01&' + values.replaceAll('&', '&amp;')
        + '&amp;end=2028-02-29' + filters + '">Download CSV</a>';
    }
  }
  return {status: 200, route, html: html + '</body>'};
}

function run(mode, json) {
  const command = JSON.parse(json);
  if (command.type === 'SubmitForm') {
    assert.equal(mode, 'host');
    hostUser = command.path === '/desktop/session/' ? null : 'User B';
    return JSON.stringify({outcome:'redirect', location: hostUser ? '/' : '/desktop/sign-in/'});
  }
  commands.push({mode, command});
  const [, slug] = reports.find(([name]) => command.report === 'reports:' + name);
  return JSON.stringify({outcome: 'export', filename: slug + '.csv', contents: body});
}

const off = () => false;
const noop = () => {};
module.exports = {
  setHost: noop, pluginsEnabled: off, bankfeedsEnabled: off, invoiceDeliveryEnabled: off,
  configureRemote: noop, configureBackups: noop, recoverBookChange: noop,
  hasCredentials: off, hostingActive: off, hostingWanted: off,
  appState: () => JSON.stringify({open: true, signed_in: true, builtin: true, session_generation: 1}),
  query: () => '[]', recurringBillsAsync: noop, aiGeneration: () => 1,
  render: target => JSON.stringify(render(target)),
  run: payload => run('local', payload),
};
const remote = require(path.join(root, 'main/remote-client.cjs'));
remote.store = () => ({
  load: () => ({host_name: 'Synthetic host'}), summary: () => ({}), usable: () => true,
});
remote.child = () => ({
  connect: async () => ({type: 'Session', signed_in: {user: 'Synthetic user'}}),
  call: async request => {
    if (request.type === 'Render') return {type: 'Page', ...render(request.target)};
    if (request.type === 'Run') return {type: 'Outcome', outcome: run('host', request.command)};
    if (request.type === 'Ping') return {type: 'Pong'};
    throw new Error('Unexpected host request: ' + request.type);
  },
  stop: noop,
});
dialog.showSaveDialog = async (window, options) => {
  assert.equal(window, BrowserWindow.getAllWindows()[0]);
  const filename = path.join(scratch, (connected ? 'host-' : 'local-') + dialogs.length + '.csv');
  dialogs.push({options, filename});
  return {canceled: false, filePath: filename};
};
function fail(error) {
  process.stderr.write(String(error.stack || error) + '\n', () => app.exit(1));
}
try { require(path.join(root, 'main/main.cjs')); } catch (error) { fail(error); }

async function until(check, detail) {
  for (let i = 0; i < 150; i++) {
    if (await check()) return;
    await delay(20);
  }
  throw new Error(detail);
}

app.whenReady().then(async () => {
  const win = BrowserWindow.getAllWindows()[0];
  const js = script => win.webContents.executeJavaScript(script);
  for (const mode of ['local', 'host']) {
    connected = mode === 'host';
    if (connected) {
      await win.loadURL('fb://app/remote/connect/?auto=1').catch(error => {
        if (!String(error.message).includes('ERR_ABORTED')) throw error;
      });
      await until(async () => win.webContents.getURL() === 'fb://app/'
        && await js('!!document.querySelector(\'meta[name="fb-remote-page"]\')'), 'Host did not connect');
    }
    for (const [name, slug, filter] of reports) {
      const page = 'fb://app/c/' + company + '/reports/' + slug + '/';
      await win.loadURL(page);
      assert.equal(await js('!!document.querySelector(\'meta[name="fb-remote-page"]\')'), connected);
      for (const [id, values] of [['plain', ['csv']], ['csv-last', ['print', 'csv']]]) {
        const index = dialogs.length;
        await js('document.getElementById(' + JSON.stringify(id) + ').click()');
        await until(() => dialogs[index] && fs.existsSync(dialogs[index].filename),
          mode + ' ' + slug + ' ' + id + ' did not reach the Save dialog');
        assert.equal(win.webContents.getURL(), page);
        assert.equal(dialogs[index].options.defaultPath, slug + '.csv');
        assert.deepEqual(dialogs[index].options.filters, [{name: 'CSV', extensions: ['csv']}]);
        assert.equal(fs.readFileSync(dialogs[index].filename, 'utf8'), body);
        assert.deepEqual(commands[index], {mode, command: {
          type: 'ExportReport', company_id: company, report: 'reports:' + name,
          filters: [['start', '2028-02-01'], ...values.map(value => ['output', value]), ['end', '2028-02-29'], ...(filter ? [filter] : [])],
        }});
      }
      const index = dialogs.length;
      await js('document.getElementById("print-last").click()');
      await until(async () => win.webContents.getURL().includes('output=csv&output=print')
        && await js('!!document.getElementById("print-view")'), 'Last output=print did not navigate');
      assert.equal(dialogs.length, index);
      assert.equal(commands.length, index);
    }
  }
  assert.equal(dialogs.length, reports.length * 4);
  // Switch users through the actual preload/main handlers on this still-connected window.
  const history = win.webContents.navigationHistory;
  const a = win.webContents.getURL();
  assert.match(a, /assets=/);
  assert.ok(new URL(a).searchParams.get('__fb_remote_page'));
  assert.equal(await js('document.getElementById("signed-in-user").textContent'), 'User A');
  assert.ok(history.canGoBack());
  await win.loadURL('fb://app/desktop/session/');
  await js('document.querySelector("form button").click()');
  await until(() => win.webContents.getURL() === 'fb://app/desktop/sign-in/', 'Sign-out did not complete');
  assert.equal(history.canGoBack(), false);
  await js('document.querySelector("form button").click()');
  await until(() => win.webContents.getURL() === 'fb://app/', 'Second sign-in did not complete');
  assert.equal(history.canGoBack(), false);
  assert.equal(history.canGoForward(), false);
  assert.equal(await js('document.getElementById("signed-in-user").textContent'), 'User B');
  assert.ok(history.getAllEntries().every(entry => !entry.url.includes('assets=')));
  const b = await js('document.querySelector(\'meta[name="fb-remote-page"]\').content');
  assert.notEqual(b, new URL(a).searchParams.get('__fb_remote_page'));
  await win.loadURL(a);
  assert.equal(win.webContents.getURL(), 'fb://app/', 'a saved marker cannot reopen the earlier worksheet');
  process.stdout.write('Wave 2b CSV clicks passed\n', () => app.exit(0));
}).catch(fail);
`;

test('CSV opens Save for all six reports locally and remotely, and a host user switch clears the previous worksheet history', {timeout: 60000}, async () => {
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'fb-wave2b-csv-'));
  try {
    const script = path.join(scratch, 'desktop.cjs');
    fs.writeFileSync(script, desktop);
    const env = {
      ...process.env,
      FB_DESKTOP_USER_DATA: path.join(scratch, 'profile'),
      FB_TEST_ELECTRON_ROOT: path.resolve(__dirname, '..'),
      FB_TEST_SCRATCH: scratch,
      FB_TEST_REPORTS: JSON.stringify(reports),
    };
    delete env.ELECTRON_RUN_AS_NODE;
    const {stdout} = await promisify(execFile)(
      require('electron'), ['--ozone-platform=headless', script], {env, timeout: 55000},
    );
    assert.match(stdout, /Wave 2b CSV clicks passed/);
  } finally {
    fs.rmSync(scratch, {recursive: true, force: true});
  }
});
