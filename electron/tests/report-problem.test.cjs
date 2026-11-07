'use strict';
// DESKTOP-JODY1: the issue page is the only outside address, opened by the main process via
// shell.openExternal after an exact-string allowlist check. No real browser is opened here.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const Module = require('node:module');
const original = Module._load;
Module._load = function(name, ...args) {
  return name === 'electron' ? {} : original.call(this, name, ...args);
};
const guards = require('../main/guards.cjs');
const about = require('../main/about.cjs');
const ISSUE = 'https://github.com/CyberJody/FreedomBooks/issues/new/choose';
const read = rel => fs.readFileSync(path.join(__dirname, '..', rel), 'utf8');

test('only the exact issue URL is allowed to open externally', () => {
  assert.deepEqual(guards.EXTERNAL_URLS, [ISSUE, 'https://bridge.simplefin.org/simplefin/create']);
  assert.equal(guards.external({url: ISSUE}), ISSUE);
  for (const url of [
    'http://github.com/CyberJody/FreedomBooks/issues/new/choose',
    'https://github.com/CyberJody/FreedomBooks/issues/new/choose/',
    'https://github.com/CyberJody/FreedomBooks/issues/new/choose?x=1',
    'https://github.com/CyberJody/FreedomBooks/issues/new/choose#x',
    'https://github.com/CyberJody/FreedomBooks/issues/new',
    'https://github.com/cyberjody/freedombooks/issues/new/choose',
    'https://github.com/Someone/FreedomBooks/issues/new/choose',
    'https://github.com/CyberJody/Other/issues/new/choose',
    'https://github.com.evil.example/CyberJody/FreedomBooks/issues/new/choose',
    'https://user@github.com/CyberJody/FreedomBooks/issues/new/choose',
    ` ${ISSUE}`, `${ISSUE}\n`, 'file:///etc/passwd', 'fb://app/', 'javascript:alert(1)', '',
  ]) assert.throws(() => guards.external({url}), /link not allowed/, url);
  for (const payload of [null, ISSUE, [ISSUE], {url: [ISSUE]}, {url: ISSUE, extra: 1}, {href: ISSUE}, {}])
    assert.throws(() => guards.external(payload), /Invalid link|link not allowed/, JSON.stringify(payload));
});

test('main process opens it via shell.openExternal behind the guard; renderer has no page API', () => {
  const main = read('main/main.cjs');
  assert.match(main, /'app\.openExternal': async \(payload\) => \{\n {4}await shell\.openExternal\(guards\.external\(payload\)\);/);
  // Two call sites: this link, and the ChatGPT sign-in (signIn), which a release build refuses first.
  assert.equal(main.match(/openExternal\(/g).length, 2, 'two call sites');
  assert.match(main, /if \(outcome\.outcome === 'external'\) \{\n {6}if \(!PLUGINS\) throw new Error\('Refused: link not allowed\.'\);\n(?: {6}\/\/[^\n]*\n)? {6}const url = guards\.signIn\(/);
  assert.match(main, /await shell\.openExternal\(url\);/);
  // The window still refuses every navigation and new window outside fb://app.
  const guardText = read('main/guards.cjs');
  assert.match(guardText, /setWindowOpenHandler\(\(\) => \(\{ action: 'deny' \}\)\)/);
  const preload = read('preload/preload.cjs');
  assert.doesNotMatch(preload, /exposeInMainWorld/);
  assert.ok(preload.includes(`const ISSUE_URL = '${ISSUE}';`));
  assert.match(preload, /call\('app\.openExternal', \{ url: href \}\)/);
  // The page links to the same address the guard allows (fb-app's ISSUE_URL feeds the template).
  assert.ok(fs.readFileSync(path.join(__dirname, '../../crates/fb-app/src/session.rs'), 'utf8').includes(`pub const ISSUE_URL: &str = "${ISSUE}";`));
  // Version and OS are handed to the core at start-up from build metadata and the host OS.
  assert.match(main, /native\.setHost\(app\.getVersion\(\), require\('\.\/about\.cjs'\)\.osLabel\(\)\);/);
});

test('OS label is the name and version only', () => {
  const files = {'/etc/os-release': 'NAME="Debian GNU/Linux"\nVERSION_ID="13"\nPRETTY_NAME="Debian GNU/Linux 13 (trixie)"\nHOME_URL="https://www.debian.org/"\n'};
  assert.equal(about.osLabel({platform: 'linux', readFile: f => files[f] || ''}), 'Debian GNU/Linux 13 (trixie)');
  // Flatpak: prefer the host's os-release over the runtime's.
  files['/run/host/os-release'] = "NAME='Linux Mint'\nVERSION_ID=22\n";
  assert.equal(about.osLabel({platform: 'linux', readFile: f => files[f] || ''}), 'Linux Mint 22');
  assert.equal(about.osLabel({platform: 'linux', readFile: () => ''}), 'Linux');
  assert.equal(about.osLabel({platform: 'darwin', systemVersion: '15.1'}), 'macOS 15.1');
  assert.equal(about.osLabel({platform: 'win32', systemVersion: '10.0.26100'}), 'Windows 10.0.26100');
});
