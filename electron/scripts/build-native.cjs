'use strict';
// Build fb-node (release) and copy it to electron/native/fb-node.node (Node-API 8, ABI-stable).
const { spawnSync } = require('node:child_process');
const { mkdirSync, copyFileSync, rmSync, writeFileSync, renameSync } = require('node:fs');
const path = require('node:path');
const {hashFile} = require('../../packaging/native-build.cjs');
const root = path.resolve(__dirname, '../..');
const manifestPath = path.join(root, 'electron/native/build-manifest.json');
rmSync(manifestPath, {force: true});
// Release builds hide the plugins: fb-node without its `plugins` feature and no fb-net helper.
// FB_PLUGINS=1 (dev/test) builds both with plugins; FB_NET_FAKE=1 (smoke only) adds the fake network.
const fake = process.env.FB_NET_FAKE === '1';
const plugins = fake || process.env.FB_PLUGINS === '1';
const simplefin = process.env.FB_SIMPLEFIN === '1';
const invoicing = fake || process.env.FB_INVOICING === '1';
// CRASHLOG: FB_PANIC_TEST=1 (tests only) builds the panic-injection hooks. They are refused in
// optimized builds, so this one is a debug build; never use it on real books.
const panicTest = process.env.FB_PANIC_TEST === '1';
const network = plugins || simplefin || invoicing;
const target = path.resolve(root, process.env.CARGO_TARGET_DIR || 'target');
const selected = [fake && 'fb-node/fake-network', plugins && 'fb-node/plugins', simplefin && 'fb-node/simplefin', invoicing && 'fb-node/invoice-delivery', panicTest && 'fb-node/panic-test'].filter(Boolean);
const features = selected.length ? ['--features', selected.join(',')] : [];
// CRASHLOG: the build identity fb-node stamps in (its build.rs): exactly this tree's commit and
// state, and today's date unless SOURCE_DATE_EPOCH says otherwise. Without git it says unknown.
const git = args => {
  const r = spawnSync('git', ['-C', root, ...args], { encoding: 'utf8' });
  return r.status === 0 ? r.stdout.trim() : null;
};
const identity = {};
const commit = git(['rev-parse', 'HEAD']);
if (commit && /^[0-9a-f]{40}$/.test(commit)) {
  identity.FB_BUILD_COMMIT = commit;
  const status = git(['status', '--porcelain']);
  if (status !== null) identity.FB_BUILD_DIRTY = status ? '1' : '0';
}
const epoch = Number(process.env.SOURCE_DATE_EPOCH);
identity.FB_BUILD_DATE = (Number.isFinite(epoch) && process.env.SOURCE_DATE_EPOCH ? new Date(epoch * 1000) : new Date()).toISOString().slice(0, 10);
const profile = panicTest ? [] : ['--release'];
const build = spawnSync('cargo', ['build', ...profile, '-p', 'fb-node', ...features], { cwd: root, stdio: 'inherit', env: { ...process.env, ...identity } });
if (build.status !== 0) process.exit(build.status || 1);
if (network) {
  // The network helper builds as a separate binary so the native module links no HTTP/TLS stack.
  const net = spawnSync('cargo', ['build', '--release', '-p', 'fb-net', ...(fake ? ['--features', 'fb-net/fake-network'] : [])], { cwd: root, stdio: 'inherit' });
  if (net.status !== 0) process.exit(net.status || 1);
}
const name = { linux: 'libfb_node.so', darwin: 'libfb_node.dylib', win32: 'fb_node.dll' }[process.platform];
if (!name) throw new Error('Unsupported OS');
mkdirSync(path.join(root, 'electron/native'), { recursive: true });
copyFileSync(path.join(target, panicTest ? 'debug' : 'release', name), path.join(root, 'electron/native/fb-node.node'));
const helper = process.platform === 'win32' ? 'fb-net.exe' : 'fb-net';
if (network) {
  copyFileSync(path.join(target, 'release', helper), path.join(root, 'electron/native', helper));
  console.log(`fb-net -> electron/native/${helper}${fake ? ' (fake-network)' : ''}`);
} else {
  rmSync(path.join(root, 'electron/native', helper), { force: true });
  console.log('plugins hidden: no fb-net helper');
}
// Remote access: the fb-remote helper beside the native module, always built. It is the TLS
// listener of "Host these books" and the connection of client mode; it never links the book.
const remoteName = process.platform === 'win32' ? 'fb-remote.exe' : 'fb-remote';
const remote = spawnSync('cargo', ['build', '--release', '-p', 'fb-remote', '--bin', 'fb-remote'], { cwd: root, stdio: 'inherit' });
if (remote.status !== 0) process.exit(remote.status || 1);
copyFileSync(path.join(target, 'release', remoteName), path.join(root, 'electron/native', remoteName));
console.log(`fb-remote -> electron/native/${remoteName}`);
console.log(`fb-node -> electron/native/fb-node.node (${process.platform}/${process.arch}${plugins ? ', plugins' : ''}${panicTest ? ', PANIC-TEST debug build: never use on real books' : ''})`);
console.log(`build identity: ${identity.FB_BUILD_COMMIT || 'unknown commit'}${identity.FB_BUILD_DIRTY === '1' ? ' (modified)' : ''}, ${identity.FB_BUILD_DATE}`);
const manifest = {
  version: 1, commit: identity.FB_BUILD_COMMIT || null, dirty: identity.FB_BUILD_DIRTY !== '0',
  platform: process.platform, arch: process.arch,
  features: {simplefin, plugins, fakeNetwork: fake, invoiceDelivery: invoicing, panicTest},
  nativeSha256: hashFile(path.join(root, 'electron/native/fb-node.node')),
  helperSha256: network ? hashFile(path.join(root, 'electron/native', helper)) : null,
  remoteSha256: hashFile(path.join(root, 'electron/native', remoteName)),
};
writeFileSync(manifestPath + '.tmp', JSON.stringify(manifest, null, 2) + '\n');
renameSync(manifestPath + '.tmp', manifestPath);
