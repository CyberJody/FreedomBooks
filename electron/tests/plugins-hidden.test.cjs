'use strict';
// Release builds hide the plugins: the IPC allow-list refuses their forms, the scheme handler their
// pages, and the session guard records (and cancels) every request that is not fb://app/.
const test = require('node:test');
const assert = require('node:assert/strict');
const Module = require('node:module');
const fs = require('node:fs');
const path = require('node:path');

const realLoad = Module._load;
Module._load = function (request, ...rest) {
  if (request === 'electron') return {};
  return realLoad.call(this, request, ...rest);
};
const guards = require('../main/guards.cjs');
const C = '0195b4a4-55af-719e-96cb-a2f946b067bf';
const form = (p) => ({ type: 'SubmitForm', path: p, token: 'ab', fields: [] });

test('plugin form posts are refused unless the build has plugins', () => {
  for (const p of [`/c/${C}/bank-feeds/connect/`, `/c/${C}/bank-feeds/${C}/sync/`, `/c/${C}/ai/`, `/c/${C}/ai/receipt/upload/`, '/auth/openai/start']) {
    assert.throws(() => guards.command(form(p)), /Unknown command/, p);
    assert.throws(() => guards.command(form(p), { plugins: false }), /Unknown command/, p);
    assert.equal(guards.command(form(p), { plugins: true }).path, p);
  }
  // Core forms, including attachments and backups, are unaffected.
  for (const p of [`/c/${C}/bills/new/`, `/c/${C}/backups/`, `/c/${C}/attachments/${C}/remove/`, `/c/${C}/aisles/`]) {
    assert.equal(guards.command(form(p)).path, p);
  }
});

test('pluginPath names only the plugin namespaces', () => {
  assert.equal(guards.pluginPath(`/c/${C}/bank-feeds/`), true);
  assert.equal(guards.pluginPath(`/c/${C}/ai/model/`), true);
  assert.equal(guards.pluginPath('/auth/openai/callback'), true);
  for (const p of [`/c/${C}/banking/`, `/c/${C}/bank-feeds`, `/c/${C}/air/`, '/auth/', null]) assert.equal(guards.pluginPath(p), false, String(p));
});

test('session guard cancels and records every request that is not fb://app/', () => {
  let handler;
  const ses = { webRequest: { onBeforeRequest: (h) => { handler = h; } }, setPermissionRequestHandler() {}, setPermissionCheckHandler() {},
    setDevicePermissionHandler() {}, on() {}, setProxy() {} };
  guards.session(ses);
  const decide = (url) => { let r; handler({ url }, (v) => { r = v; }); return r.cancel; };
  assert.equal(decide('fb://app/c/x/home/'), false);
  assert.equal(decide('https://example.com/'), true);
  assert.equal(decide('http://127.0.0.1:9/'), true);
  const log = guards.requestLog();
  assert.equal(log.fb, 1);
  assert.deepEqual(log.refused, ['https://example.com/', 'http://127.0.0.1:9/']);
});

test('session guard switches off the spellchecker and its dictionary download', () => {
  const calls = [];
  const ses = { webRequest: { onBeforeRequest() {} }, setPermissionRequestHandler() {}, setPermissionCheckHandler() {},
    setDevicePermissionHandler() {}, on() {}, setProxy() {},
    setSpellCheckerEnabled: (v) => calls.push(['enabled', v]), setSpellCheckerLanguages: (v) => calls.push(['languages', v]) };
  guards.session(ses);
  assert.deepEqual(calls, [['enabled', false], ['languages', []]]);
});

test('the main process keys every plugin surface off the compiled flag', () => {
  const main = fs.readFileSync(path.join(__dirname, '../main/main.cjs'), 'utf8');
  assert.match(main, /const PLUGINS = native\.pluginsEnabled\(\) === true;/);
  assert.match(main, /if \(PLUGINS \|\| BANK_FEEDS\) \{\n {2}const helper = development\.netHelper/);
  assert.match(main, /guards\.command\(payload, \{ plugins: PLUGINS, bankFeeds: BANK_FEEDS \}\)/);
  assert.match(main, /if \(!guards\.pluginAllowed\(url\.pathname, \{plugins:PLUGINS,bankFeeds:BANK_FEEDS\}\)\) return new Response\('Not found', \{ status: 404 \}\);/);
  // Provider traffic stays in fb-net. The opt-in local agent listener is the only HTTP server;
  // it has no outbound client and is independently covered by adversarial transport tests.
  for (const file of fs.readdirSync(path.join(__dirname, '../main'))) {
    const text = fs.readFileSync(path.join(__dirname, '../main', file), 'utf8');
    if(file==='ai-transport.cjs') {
      assert.match(text,/server\.listen\(0,'127\.0\.0\.1',resolve\)/);
      assert.doesNotMatch(text,/http\.(request|get)\(|\bfetch\(/);
      continue;
    }
    assert.doesNotMatch(text, /require\('node:(https?|net|tls|dgram)'\)|require\('(https?|net|tls|dgram)'\)|\bnet\.(fetch|request)\(/, file);
  }
  const build = fs.readFileSync(path.join(__dirname, '../scripts/build-native.cjs'), 'utf8');
  assert.match(build, /if \(network\) \{\n {2}\/\/ The network helper builds/);
});
