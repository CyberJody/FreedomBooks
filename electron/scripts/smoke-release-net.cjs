'use strict';
// Release build, plugins hidden (FB_SMOKE_NETLOG set): after the full smoke, nothing of a plugin is
// reachable and nothing left the machine. Two independent records: the session guard's own log of
// every request (only fb://app/ allowed; every other address is one of the smoke's deliberate
// probes, and was cancelled), and Chromium's net log of the whole run (no socket, no DNS lookup).
const assert = require('node:assert/strict');
const fs = require('node:fs');

// The smoke's deliberate refusal probes (fetch, window.open and navigation to example.com; the
// WebRTC check's loopback fetch). Anything else is an outbound attempt.
const PROBES = [/^https:\/\/example\.com\/$/, /^http:\/\/127\.0\.0\.1:[0-9]+\/$/];

async function live({ app, page, company, pass }) {
  const state = await app.evaluate(async ({ app: a, ipcMain, BrowserWindow }, companyId) => {
    const require = process.getBuiltinModule('module').createRequire(a.getAppPath() + '/package.json');
    const path = require('node:path');
    const fs = require('node:fs');
    const guards = require(path.join(a.getAppPath(), 'main', 'guards.cjs'));
    const native = require(path.join(a.getAppPath(), 'native', 'fb-node.node'));
    const contents = BrowserWindow.getAllWindows()[0].webContents;
    const handler = ipcMain._invokeHandlers.get('fb:books.run');
    const event = { sender: contents, senderFrame: contents.mainFrame };
    const refused = [];
    for (const p of ['bank-feeds/connect/', 'ai/', 'ai/receipt/upload/']) {
      const result = await handler(event, { type: 'SubmitForm', path: `/c/${companyId}/${p}`, token: '00', fields: [] });
      refused.push(!result.ok && /Unknown command/.test(result.error));
    }
    return {
      plugins: native.pluginsEnabled(),
      origins: JSON.parse(native.externalOrigins()),
      helper: ['fb-net', 'fb-net.exe'].some(n => fs.existsSync(path.join(a.getAppPath(), 'native', n))),
      refused,
      log: guards.requestLog(),
    };
  }, company);
  assert.equal(state.plugins, false, 'release build: plugins off');
  assert.deepEqual(state.origins, [], 'no sign-in origin');
  assert.equal(state.helper, false, 'no fb-net helper beside the native module');
  assert.deepEqual(state.refused, [true, true, true], 'plugin forms refused at the IPC allow-list');
  pass('release build: plugins off, no network helper, no sign-in origin, plugin forms refused over IPC');

  for (const p of ['bank-feeds/', 'bank-feeds/rules/', 'ai/']) {
    const response = await page.goto(`fb://app/c/${company}/${p}`);
    assert.equal(response.status(), 404, p);
  }
  await page.goto(`fb://app/c/${company}/home/`);
  const nav = await page.locator('nav.appnav').textContent();
  const footer = await page.locator('footer').textContent();
  for (const word of ['Bank feeds', 'AI assistant']) assert.ok(!nav.includes(word) && !footer.includes(word), word);
  const menu = await app.evaluate(({ Menu }) => JSON.stringify(Menu.getApplicationMenu().items.map(function walk(i) { return { l: i.label, s: (i.submenu ? i.submenu.items : []).map(walk) }; })));
  assert.doesNotMatch(menu, /bank|feed|chatgpt|assistant|openai|receipt/i, 'application menu');
  pass('plugin pages answer 404; nav, footer and menus name no plugin');

  const log = await app.evaluate(({ app: a }) => {
    const require = process.getBuiltinModule('module').createRequire(a.getAppPath() + '/package.json');
    return require(require('node:path').join(a.getAppPath(), 'main', 'guards.cjs')).requestLog();
  });
  const unexpected = log.refused.filter(url => !PROBES.some(p => p.test(url)));
  assert.ok(log.fb > 100, `fb://app/ requests seen: ${log.fb}`);
  assert.deepEqual(unexpected, [], 'only fb://app/ and the smoke probes were requested');
  console.log(`      session guard: ${log.fb} fb://app/ requests; ${log.refused.length} refused probe(s): ${[...new Set(log.refused)].join(', ')}`);
  pass('session guard: every request was fb://app/ except the deliberate probes, all cancelled');
}

// After app.close: Chromium's net log of the whole run.
function netlog(file) {
  const raw = fs.readFileSync(file, 'utf8');
  // The log is complete once the app has quit; tolerate a missing final bracket.
  let log;
  try { log = JSON.parse(raw); } catch { log = JSON.parse(raw.replace(/,\s*$/, '') + ']}'); }
  const names = Object.fromEntries(Object.entries(log.constants.logEventTypes).map(([k, v]) => [v, k]));
  const types = {};
  const urls = new Set();
  for (const e of log.events) {
    const name = names[e.type] || String(e.type);
    types[name] = (types[name] || 0) + 1;
    if (e.params && typeof e.params.url === 'string') urls.add(e.params.url);
  }
  // Events that mean a lookup, socket or session happened (config-change notices do not).
  const CONNECTION = /^(HOST_RESOLVER_MANAGER_(REQUEST|JOB)|HOST_RESOLVER_SYSTEM_TASK|HOST_RESOLVER_DNS_TASK|TCP_CONNECT(_ATTEMPT)?|UDP_CONNECT|SOCKET_OPEN|SOCKET_CONNECT|SOCKET_BYTES_SENT|UDP_BYTES_SENT|SSL_CONNECT|QUIC_SESSION|QUIC_SESSION_PACKET_SENT|HTTP2_SESSION|URL_REQUEST_DELEGATE_CONNECTED|PROXY_SCRIPT_DECIDER)$/;
  const network = Object.keys(types).filter(n => CONNECTION.test(n));
  const outside = [...urls].filter(u => !u.startsWith('fb://app/') && !PROBES.some(p => p.test(u)));
  assert.deepEqual(outside, [], 'net log: no request to any address but fb://app/ and the probes');
  assert.deepEqual(network.map(n => `${n}=${types[n]}`), [], 'net log: no socket, connect, DNS, TLS or QUIC event');
  console.log(`      net log: ${log.events.length} events, ${urls.size} URL(s), none outside fb://app/ but the probes; 0 socket/DNS/TLS events`);
  return log.events.length;
}

module.exports = { live, netlog };
