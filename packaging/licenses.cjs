// Preserve runtime notices byte-for-byte; never substitute our future license for Electron's.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
async function auditInput(stage, platform) {
  const checker = require('license-checker');
  const result = await new Promise((resolve, reject) => checker.init({start: stage, production: true}, (error, data) => error ? reject(error) : resolve(data)));
  // There must be no shipped npm dependency until its license is explicitly audited.
  assert.deepEqual(Object.keys(result).filter(name => !name.startsWith('freedombooks@')), [], 'unexpected npm production module in ASAR input');
  assert.ok(!fs.existsSync(path.join(stage, 'node_modules')), 'dev-only modules must not ship');
  const output = path.join(__dirname, 'dist');
  fs.mkdirSync(output, {recursive: true});
  fs.writeFileSync(path.join(output, `npm-production-${platform}.json`), JSON.stringify(result, null, 2) + '\n');
  console.log(`LICENSE-CHECKER --production ${platform}: 0 third-party npm production dependencies (exact ASAR input)`);
}
function installNotices(buildPath) {
  const destination = path.join(buildPath, 'licenses/electron');
  fs.mkdirSync(destination, {recursive: true});
  // Packager renames Electron's MIT notice on Windows to LICENSE.electron.txt.
  for (const name of ['LICENSE', 'LICENSES.chromium.html']) {
    const source = [name, name === 'LICENSE' ? 'LICENSE.electron.txt' : name].map(n => path.join(buildPath, n)).find(p => fs.existsSync(p));
    assert.ok(source, `missing upstream Electron notice ${name}`);
    fs.copyFileSync(source, path.join(destination, name));
  }
  fs.copyFileSync(path.join(root, 'THIRD-PARTY-NOTICES.md'), path.join(buildPath, 'THIRD-PARTY-NOTICES.md'));
  if (fs.existsSync(path.join(root, 'LICENSE'))) {
    fs.mkdirSync(path.join(buildPath, 'licenses/freedombooks'), {recursive: true});
    fs.copyFileSync(path.join(root, 'LICENSE'), path.join(buildPath, 'licenses/freedombooks/LICENSE'));
  } else console.log('LOCAL UNSIGNED BUILD: project LICENSE awaits publication; release CI requires it.');
}
module.exports = {auditInput, installNotices};
