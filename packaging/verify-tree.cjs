'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {verify} = require('./fuses.cjs');
(async () => {
  const [tree, reference, platform, incomplete] = process.argv.slice(2);
  const binary = platform === 'win32' ? 'FreedomBooks.exe' : 'freedombooks';
  await verify(path.join(tree, binary));
  for (const name of [binary, 'resources/app.asar', ...(incomplete === 'incomplete-shell' ? [] : ['resources/app.asar.unpacked/native/fb-node.node'])]) {
    assert.ok(fs.readFileSync(path.join(tree, name)).equals(fs.readFileSync(path.join(reference, name))), `installer changed hardened input ${name}`);
  }
  const {listPackage, extractFile} = await import('@electron/asar');
  const asar = path.join(tree, 'resources/app.asar');
  const entries = listPackage(asar);
  assert.ok(!entries.some(name => /\/(node_modules|tests|scripts|staging|\.tooling)(\/|$)/.test(name)), 'ASAR must contain only release input');
  const metadata = JSON.parse(extractFile(asar, 'package.json'));
  const hasSimplefin = metadata.freedomBooksFeatures?.simplefin === true;
  const helper = platform === 'win32' ? 'fb-net.exe' : 'fb-net';
  const unpacked = path.join(tree, 'resources/app.asar.unpacked/native');
  const expected = incomplete === 'incomplete-shell' ? [] : ['fb-node.node', ...(hasSimplefin ? [helper] : [])];
  if (fs.existsSync(unpacked)) assert.deepEqual(fs.readdirSync(unpacked).filter(n => n !== 'PENDING.txt').sort(), expected.sort(), 'unpacked native allowlist');
  const helpers = entries.filter(name => /(^|\/)fb-net(\.exe)?$/.test(name));
  assert.equal(helpers.length, hasSimplefin ? 1 : 0, 'helper matches declared capabilities');
  if (hasSimplefin) {
    assert.ok(fs.readFileSync(path.join(unpacked, helper)).equals(fs.readFileSync(path.join(reference, 'resources/app.asar.unpacked/native', helper))), 'installer changed network helper');
    if (platform !== 'win32') assert.ok(fs.statSync(path.join(unpacked, helper)).mode & 0o100, 'helper must be executable');
  }
  assert.ok(!metadata.devDependencies && !metadata.scripts && !metadata.dependencies);
  assert.equal(metadata.desktopName, 'app.freedombooks.FreedomBooks.desktop');
  assert.equal(Boolean(metadata.freedomBooksIncomplete), incomplete === 'incomplete-shell');
  if (platform === 'win32') {
    const {NtExecutable, NtExecutableResource} = await import('resedit');
    const {createHash} = require('node:crypto');
    const {getRawHeader} = await import('@electron/asar');
    const resources = NtExecutableResource.from(NtExecutable.from(fs.readFileSync(path.join(tree, binary))));
    const entries = resources.entries.filter(entry => entry.type === 'INTEGRITY' && entry.id === 'ELECTRONASAR');
    assert.equal(entries.length, 1, 'PE must embed one Electron ASAR integrity resource');
    const integrity = JSON.parse(Buffer.from(entries[0].bin).toString('utf8'));
    assert.deepEqual(integrity, [{file: 'resources\\app.asar', alg: 'SHA256', value: createHash('sha256').update(getRawHeader(asar).headerString).digest('hex')}]);
    console.log('WINDOWS EMBEDDED ASAR INTEGRITY VERIFIED');
  }
  console.log(`FUSES/ASAR VERIFIED ${platform}/x64 ${tree}`);
})().catch(error => {console.error(error); process.exitCode = 1;});
