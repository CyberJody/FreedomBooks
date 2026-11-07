'use strict';
// Build fb-node (release) and copy it to electron/native/fb-node.node (Node-API 8, ABI-stable).
const { spawnSync } = require('node:child_process');
const { mkdirSync, copyFileSync, rmSync } = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '../..');
// Release builds hide the plugins: fb-node without its `plugins` feature and no fb-net helper.
// FB_PLUGINS=1 (dev/test) builds both with plugins; FB_NET_FAKE=1 (smoke only) adds the fake network.
const fake = process.env.FB_NET_FAKE === '1';
const plugins = fake || process.env.FB_PLUGINS === '1';
const simplefin = process.env.FB_SIMPLEFIN === '1';
const network = plugins || simplefin;
const target = path.resolve(root, process.env.CARGO_TARGET_DIR || 'target');
const features = fake ? ['--features', 'fb-node/fake-network'] : plugins ? ['--features', 'fb-node/plugins'] : simplefin ? ['--features', 'fb-node/simplefin'] : [];
const build = spawnSync('cargo', ['build', '--release', '-p', 'fb-node', ...features], { cwd: root, stdio: 'inherit' });
if (build.status !== 0) process.exit(build.status || 1);
if (network) {
  // The network helper builds as a separate binary so the native module links no HTTP/TLS stack.
  const net = spawnSync('cargo', ['build', '--release', '-p', 'fb-net', ...(fake ? ['--features', 'fb-net/fake-network'] : [])], { cwd: root, stdio: 'inherit' });
  if (net.status !== 0) process.exit(net.status || 1);
}
const name = { linux: 'libfb_node.so', darwin: 'libfb_node.dylib', win32: 'fb_node.dll' }[process.platform];
if (!name) throw new Error('Unsupported OS');
mkdirSync(path.join(root, 'electron/native'), { recursive: true });
copyFileSync(path.join(target, 'release', name), path.join(root, 'electron/native/fb-node.node'));
const helper = process.platform === 'win32' ? 'fb-net.exe' : 'fb-net';
if (network) {
  copyFileSync(path.join(target, 'release', helper), path.join(root, 'electron/native', helper));
  console.log(`fb-net -> electron/native/${helper}${fake ? ' (fake-network)' : ''}`);
} else {
  rmSync(path.join(root, 'electron/native', helper), { force: true });
  console.log('plugins hidden: no fb-net helper');
}
console.log(`fb-node -> electron/native/fb-node.node (${process.platform}/${process.arch}${plugins ? ', plugins' : ''})`);
