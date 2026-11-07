'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const fs = require('node:fs');
const identity = require('../main/identity.cjs');

test('legacy and current launches use the same lock namespace before migration', () => {
  for (const claimed of [true, false]) {
    const calls = [];
    let userData;
    const app = {
      getPath: key => { assert.equal(key, 'appData'); return '/sandbox/config'; },
      setPath: (key, value) => { calls.push(['setPath', key, value]); userData = value; },
      requestSingleInstanceLock: () => { calls.push(['lock', userData]); return claimed; },
    };
    assert.equal(identity.claimInstance(app, '/sandbox/settings'), claimed);
    assert.deepEqual(calls, [
      ['setPath', 'userData', '/sandbox/config/FreedomBooks'],
      ['lock', '/sandbox/config/FreedomBooks'],
      ['setPath', 'userData', '/sandbox/settings'],
    ]);
  }
  const main = fs.readFileSync(path.join(__dirname, '../main/main.cjs'), 'utf8');
  assert.doesNotMatch(main, /requestSingleInstanceLock|setPath\('userData'/);
  assert.ok(main.indexOf('claimInstance(') < main.indexOf('keys.userDataDir(app)'));
});

for (const platform of ['linux', 'win32', 'darwin']) {
  test(`permanent app identity and data directory on ${platform}`, () => {
    const calls = [];
    const app = {
      setName: (v) => calls.push(['name', v]),
      setAppUserModelId: (v) => calls.push(['aumid', v]),
      setDesktopName: (v) => calls.push(['desktop', v]),
      commandLine: { appendSwitch: (k, v) => calls.push([k, v]) },
      getPath: (key) => { assert.equal(key, 'appData'); return path.resolve('app-data'); },
    };
    identity.configure(app, platform);
    assert.equal(identity.ID, 'app.freedombooks.FreedomBooks');
    assert.equal(identity.settingsDir(app), path.resolve('app-data', identity.ID));
    assert.equal(identity.dataDir(app, platform, {XDG_DATA_HOME: '/sandbox/data'}), platform === 'linux'
      ? '/sandbox/data/FreedomBooks' : path.resolve('app-data', identity.ID));
    assert.deepEqual(calls, platform === 'linux'
      ? [['name', 'FreedomBooks'], ['desktop', `${identity.ID}.desktop`], ['class', identity.ID]]
      : platform === 'win32' ? [['name', 'FreedomBooks'], ['aumid', identity.ID]] : [['name', 'FreedomBooks']]);
  });
}
