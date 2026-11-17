'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const {createHash} = require('node:crypto');

const hashFile = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');

function validateNativeBuild(manifest, {native, helper, remote, platform, arch, commit, simplefin}) {
  assert.equal(manifest.version, 1, 'native build manifest version');
  assert.match(manifest.commit || '', /^[0-9a-f]{40}$/, 'native build commit');
  assert.equal(manifest.commit, commit, 'rebuild native module for the current commit');
  assert.equal(manifest.dirty, false, 'rebuild native module from a clean committed tree');
  assert.equal(manifest.platform, platform, 'native build platform');
  assert.equal(manifest.arch, arch, 'native build architecture');
  assert.deepEqual(Object.keys(manifest.features || {}).sort(), ['fakeNetwork', 'invoiceDelivery', 'panicTest', 'plugins', 'simplefin']);
  for (const [feature, enabled] of Object.entries(manifest.features)) {
    assert.equal(typeof enabled, 'boolean', `native feature ${feature}`);
    if (feature !== 'simplefin') assert.equal(enabled, false, `non-production native feature: ${feature}`);
  }
  if (simplefin !== undefined) assert.equal(manifest.features.simplefin, simplefin, 'SimpleFIN selection must match the native build');
  assert.equal(hashFile(native), manifest.nativeSha256, 'native module does not match its build manifest');
  if (manifest.features.simplefin) {
    assert.ok(helper && fs.existsSync(helper), 'SimpleFIN requires the native network helper');
    assert.equal(hashFile(helper), manifest.helperSha256, 'network helper does not match its build manifest');
  } else {
    assert.equal(manifest.helperSha256, null, 'offline native build must not declare a helper');
    assert.ok(!helper, 'offline native build must not package a helper');
  }
  // Remote access ("Host these books" and client mode): every build carries the fb-remote helper.
  assert.match(manifest.remoteSha256 || '', /^[0-9a-f]{64}$/, 'native build must include the remote-access helper');
  assert.ok(remote && fs.existsSync(remote), 'the remote-access helper is missing');
  assert.equal(hashFile(remote), manifest.remoteSha256, 'remote-access helper does not match its build manifest');
  return manifest;
}

module.exports = {hashFile, validateNativeBuild};
