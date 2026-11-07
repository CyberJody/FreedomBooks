'use strict';
const assert = require('node:assert/strict');
const required = Object.freeze({
  RunAsNode: false,
  EnableNodeCliInspectArguments: false,
  EnableNodeOptionsEnvironmentVariable: false,
  OnlyLoadAppFromAsar: true,
  EnableEmbeddedAsarIntegrityValidation: true,
});
async function verify(binary) {
  const {getCurrentFuseWire, FuseVersion, FuseV1Options, FuseState} = await import('@electron/fuses');
  const wire = await getCurrentFuseWire(binary);
  assert.equal(wire.version, FuseVersion.V1);
  for (const [name, on] of Object.entries(required)) {
    assert.equal(wire[FuseV1Options[name]], on ? FuseState.ENABLE : FuseState.DISABLE, `fuse ${name} on ${binary}`);
  }
  return wire;
}
async function harden(binary) {
  const {flipFuses, FuseVersion, FuseV1Options} = await import('@electron/fuses');
  const config = Object.fromEntries(Object.entries(required).map(([name,on]) => [FuseV1Options[name],on]));
  await flipFuses(binary, {version: FuseVersion.V1, ...config});
  await verify(binary);
}
module.exports = { harden, verify, required };
