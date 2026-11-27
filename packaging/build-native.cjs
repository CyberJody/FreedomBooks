'use strict';
// Shared producer for the desktop and release installers. The manifest is published last.
const {execFileSync, spawnSync} = require('node:child_process');
const {mkdirSync, copyFileSync, rmSync, writeFileSync, renameSync} = require('node:fs');
const path = require('node:path');
const {hashFile} = require('./native-build.cjs');

function buildNative(root, {platform = process.platform, arch = process.arch, crossWindows = false, output}) {
  const manifestPath = path.join(output, 'build-manifest.json');
  rmSync(manifestPath, {force: true});
  const fake = process.env.FB_NET_FAKE === '1';
  const plugins = fake || process.env.FB_PLUGINS === '1';
  const simplefin = process.env.FB_SIMPLEFIN === '1';
  const invoicing = fake || process.env.FB_INVOICING === '1';
  const panicTest = process.env.FB_PANIC_TEST === '1';
  const network = plugins || simplefin || invoicing;
  const features = [fake && 'fb-node/fake-network', plugins && 'fb-node/plugins', simplefin && 'fb-node/simplefin',
    invoicing && 'fb-node/invoice-delivery', panicTest && 'fb-node/panic-test'].filter(Boolean);
  const git = args => {
    const r = spawnSync('git', args, {cwd: root, encoding: 'utf8'});
    return r.status === 0 ? r.stdout.trim() : null;
  };
  const commit = git(['rev-parse', 'HEAD']);
  const status = commit ? git(['status', '--porcelain']) : null;
  const dirty = status !== '';
  const epoch = Number(process.env.SOURCE_DATE_EPOCH);
  const date = (Number.isFinite(epoch) && process.env.SOURCE_DATE_EPOCH ? new Date(epoch * 1000) : new Date()).toISOString().slice(0, 10);
  const env = {...process.env, FB_BUILD_DATE: date};
  if (commit) env.FB_BUILD_COMMIT = commit;
  if (status !== null) env.FB_BUILD_DIRTY = dirty ? '1' : '0';
  const triple = 'x86_64-pc-windows-msvc';
  const target = path.resolve(root, env.CARGO_TARGET_DIR || 'target', crossWindows ? triple : '');
  function cargoBuild(args) {
    const command = [...(crossWindows ? ['xwin'] : []), 'build', '--locked', ...(crossWindows ? ['--target', triple] : []), ...args];
    execFileSync('cargo', command, {cwd: root, env, stdio: 'inherit'});
  }

  // Panic injection is only supported in debug builds. Helpers always build in release mode.
  cargoBuild([...(panicTest ? [] : ['--release']), '-p', 'fb-node', ...(features.length ? ['--features', features.join(',')] : [])]);
  cargoBuild(['--release', '-p', 'fb-remote', '--bin', 'fb-remote']);
  if (network) cargoBuild(['--release', '-p', 'fb-net', '--bin', 'fb-net', ...(fake ? ['--features', 'fb-net/fake-network'] : [])]);

  const library = {linux: 'libfb_node.so', darwin: 'libfb_node.dylib', win32: 'fb_node.dll'}[platform];
  const helper = platform === 'win32' ? 'fb-net.exe' : 'fb-net';
  const remote = platform === 'win32' ? 'fb-remote.exe' : 'fb-remote';
  mkdirSync(output, {recursive: true});
  const copy = (profile, source, name) => {
    const dest = path.join(output, name);
    copyFileSync(path.join(target, profile, source), dest);
    return hashFile(dest);
  };
  const manifest = {
    version: 1, commit, dirty, platform, arch,
    features: {simplefin, plugins, fakeNetwork: fake, invoiceDelivery: invoicing, panicTest},
    nativeSha256: copy(panicTest ? 'debug' : 'release', library, 'fb-node.node'),
    remoteSha256: copy('release', remote, remote),
    helperSha256: network ? copy('release', helper, helper) : null,
  };
  if (!network) rmSync(path.join(output, helper), {force: true});
  writeFileSync(manifestPath + '.tmp', JSON.stringify(manifest, null, 2) + '\n');
  renameSync(manifestPath + '.tmp', manifestPath);
  console.log(`fb-node -> ${path.relative(root, path.join(output, 'fb-node.node'))} (${platform}/${arch})`);
  console.log(`Native build ${platform}/${arch}: ${commit || 'unknown'}${dirty ? ' (modified)' : ''}, ${date} -> ${output}`);
}

if (require.main === module) {
  const platform = process.argv[2];
  if (!['linux', 'win32'].includes(platform)) throw new Error('expected release platform linux or win32');
  const crossWindows = platform === 'win32' && process.platform !== 'win32';
  if (!crossWindows && (process.platform !== platform || process.arch !== 'x64')) throw new Error('expected an x64 release host for ' + platform);
  const root = path.resolve(__dirname, '..');
  const output = path.join(root, 'electron/native', platform === 'win32' ? 'win32-x64' : '');
  buildNative(root, {platform, arch: 'x64', crossWindows, output});
}
module.exports = {buildNative};
