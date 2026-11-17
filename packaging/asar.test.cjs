'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {NATIVE_UNPACK, cliOptions} = require('./package.cjs');

test('native module is unpacked beneath hidden CI temporary directories', async () => {
  const {createPackageWithOptions, statFile} = await import('@electron/asar');
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'fb-asar-'));
  try {
    const input = path.join(temp, '.work/tmp/app');
    fs.mkdirSync(path.join(input, 'native'), {recursive: true});
    fs.writeFileSync(path.join(input, 'native/fb-node.node'), 'native fixture');
    fs.writeFileSync(path.join(input, 'native/fb-net'), 'helper fixture', {mode:0o755});
    fs.writeFileSync(path.join(input, 'native/fb-remote'), 'remote helper fixture', {mode:0o755});
    const archive = path.join(temp, 'app.asar');
    await createPackageWithOptions(input, archive, {unpack: NATIVE_UNPACK});
    assert.equal(statFile(archive, 'native/fb-node.node').unpacked, true);
    assert.equal(fs.readFileSync(`${archive}.unpacked/native/fb-node.node`, 'utf8'), 'native fixture');
    assert.equal(statFile(archive, 'native/fb-net').unpacked, true);
    assert.equal(fs.readFileSync(`${archive}.unpacked/native/fb-net`, 'utf8'), 'helper fixture');
    assert.ok(fs.statSync(`${archive}.unpacked/native/fb-net`).mode & 0o100);
    assert.equal(statFile(archive, 'native/fb-remote').unpacked, true, 'the remote-access helper runs from the unpacked folder');
    assert.ok(fs.statSync(`${archive}.unpacked/native/fb-remote`).mode & 0o100);
  } finally {fs.rmSync(temp, {recursive: true, force: true});}
});

test('CLI defaults to native capabilities and can require a SimpleFIN selection', () => {
  assert.deepEqual(cliOptions({}), {});
  assert.deepEqual(cliOptions({FB_SIMPLEFIN: '0'}), {simplefin: false});
  assert.deepEqual(cliOptions({FB_SIMPLEFIN: '1'}), {simplefin: true});
});
