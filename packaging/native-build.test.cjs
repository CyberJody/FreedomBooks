'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const {hashFile, validateNativeBuild} = require('./native-build.cjs');

test('release manifest binds capabilities, source and both native binaries', () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'fb-native-manifest-'));
  try {
    const native = path.join(temp, 'fb-node.node'), helper = path.join(temp, 'fb-net'), remote = path.join(temp, 'fb-remote');
    fs.writeFileSync(native, 'native fixture');
    fs.writeFileSync(helper, 'production helper fixture');
    fs.writeFileSync(remote, 'remote-access helper fixture');
    const options = {native, helper, remote, platform: 'linux', arch: 'x64', commit: 'a'.repeat(40)};
    const manifest = {version: 1, commit: options.commit, dirty: false, platform: 'linux', arch: 'x64',
      features: {simplefin: true, plugins: false, fakeNetwork: false, invoiceDelivery: false, panicTest: false},
      nativeSha256: hashFile(native), helperSha256: hashFile(helper), remoteSha256: hashFile(remote)};
    assert.equal(validateNativeBuild(manifest, options), manifest);
    assert.throws(() => validateNativeBuild(manifest, {...options, simplefin: false}), /SimpleFIN selection/);
    assert.throws(() => validateNativeBuild({...manifest, dirty: true}, options), /clean committed/);
    assert.throws(() => validateNativeBuild({...manifest, commit: 'b'.repeat(40)}, options), /current commit/);
    assert.throws(() => validateNativeBuild(manifest, {...options, arch: 'arm64'}), /architecture/);
    for (const feature of ['plugins', 'fakeNetwork', 'invoiceDelivery', 'panicTest']) {
      assert.throws(() => validateNativeBuild({...manifest, features: {...manifest.features, [feature]: true}}, options), /non-production/);
    }
    assert.throws(() => validateNativeBuild({...manifest, nativeSha256: 'bad'}, options), /native module does not match/);
    fs.writeFileSync(helper, 'leftover fake helper');
    assert.throws(() => validateNativeBuild(manifest, options), /network helper does not match/);
    fs.unlinkSync(helper);
    assert.throws(() => validateNativeBuild(manifest, options), /requires the native network helper/);
    const offline = {...manifest, features: {...manifest.features, simplefin: false}, helperSha256: null};
    assert.equal(validateNativeBuild(offline, {...options, helper: undefined}), offline);
    assert.throws(() => validateNativeBuild(offline, options), /must not package a helper/);
    // The remote-access helper is in every build and must be the one built.
    assert.throws(() => validateNativeBuild({...offline, remoteSha256: null}, {...options, helper: undefined}), /must include the remote-access helper/);
    assert.throws(() => validateNativeBuild(offline, {...options, helper: undefined, remote: undefined}), /remote-access helper is missing/);
    fs.writeFileSync(remote, 'a different helper');
    assert.throws(() => validateNativeBuild(offline, {...options, helper: undefined}), /remote-access helper does not match/);
  } finally { fs.rmSync(temp, {recursive: true, force: true}); }
});
