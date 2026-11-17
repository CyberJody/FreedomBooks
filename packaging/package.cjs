// Whitelist the ASAR input; builder only wraps this already hardened application.
'use strict';
const path = require('node:path');
const fs = require('node:fs');
const os = require('node:os');
const assert = require('node:assert/strict');
const {execFileSync} = require('node:child_process');
const {validateNativeBuild} = require('./native-build.cjs');
const { harden } = require('./fuses.cjs');
const { auditInput, installNotices } = require('./licenses.cjs');
const root = path.resolve(__dirname, '..');
const NATIVE_UNPACK = '{*.node,fb-net,fb-net.exe,fb-remote,fb-remote.exe}';
async function build(options = {}) {
  const { packager } = await import('@electron/packager');
  const platform = options.platform || process.platform;
  const arch = options.arch || 'x64';
  const native = options.native || path.join(root, 'electron/native/fb-node.node');
  let netHelper = options.netHelper;
  const remoteHelper = options.remoteHelper || path.join(path.dirname(native), platform === 'win32' ? 'fb-remote.exe' : 'fb-remote');
  let nativeBuild;
  const incomplete = !fs.existsSync(native) && options.allowMissingNative === true && platform === 'win32';
  if (!fs.existsSync(native) && !incomplete) throw new Error(`Build fb-node for ${platform}/${arch} first: ${native}`);
  if (!incomplete) {
    assert.equal(execFileSync('git', ['status', '--porcelain'], {cwd: root, encoding: 'utf8'}).trim(), '', 'package only a clean committed tree');
    const manifest = JSON.parse(fs.readFileSync(options.nativeManifest || path.join(path.dirname(native), 'build-manifest.json'), 'utf8'));
    if (manifest.features?.simplefin) netHelper ||= path.join(path.dirname(native), platform === 'win32' ? 'fb-net.exe' : 'fb-net');
    nativeBuild = validateNativeBuild(manifest, {
      native, helper: netHelper, remote: remoteHelper, platform, arch, simplefin: options.simplefin,
      commit: execFileSync('git', ['rev-parse', 'HEAD'], {cwd: root, encoding: 'utf8'}).trim(),
    });
    const header = fs.readFileSync(native).subarray(0, 4);
    assert.ok(platform === 'win32' ? header.subarray(0, 2).toString() === 'MZ' : header.toString('hex') === '7f454c46', 'native module target header');
  }
  const stage = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-package-'));
  const metadata = JSON.parse(fs.readFileSync(path.join(root, 'electron/package.json')));
  metadata.version = options.version || metadata.version;
  delete metadata.devDependencies;
  delete metadata.scripts;
  delete metadata.build;
  metadata.description = 'FreedomBooks accounting';
  metadata.author = {name: 'FreedomBooks'};
  metadata.name = 'freedombooks';
  metadata.desktopName = 'app.freedombooks.FreedomBooks.desktop';
  metadata.main = 'main/install-entry.cjs';
  metadata.freedomBooksFeatures = {simplefin: nativeBuild?.features.simplefin === true};
  if (nativeBuild) metadata.freedomBooksNativeBuild = nativeBuild;
  if (incomplete) metadata.freedomBooksIncomplete = 'Windows fb-node native module not built; this developer shell is NOT runnable or releasable.';
  try {
    for (const dir of ['main', 'preload', 'renderer', 'agent']) fs.cpSync(path.join(root, 'electron', dir), path.join(stage, dir), {recursive: true});
    // SQLite respects the process umask when it first creates its encrypted
    // database/WAL. Keep packaged first-run files private even with a 022 login.
    fs.writeFileSync(path.join(stage, 'main/install-entry.cjs'), "'use strict';\nif (process.platform !== 'win32') process.umask(0o077);\nrequire('./entry.cjs');\n");
    fs.mkdirSync(path.join(stage, 'native'));
    if (incomplete) fs.writeFileSync(path.join(stage, 'native/PENDING.txt'), metadata.freedomBooksIncomplete + '\n');
    else fs.copyFileSync(native, path.join(stage, 'native/fb-node.node'));
    if (netHelper) {
      assert.ok(!incomplete && fs.statSync(netHelper).isFile(), 'network helper requires a complete native build');
      const header = fs.readFileSync(netHelper).subarray(0, 4);
      assert.ok(platform === 'win32' ? header.subarray(0, 2).toString() === 'MZ' : header.toString('hex') === '7f454c46', 'network helper target header');
      const helper = path.join(stage, 'native', platform === 'win32' ? 'fb-net.exe' : 'fb-net');
      fs.copyFileSync(netHelper, helper);
      fs.chmodSync(helper, 0o755);
    }
    if (!incomplete) {
      // "Host these books" and client mode: the fb-remote helper at its fixed path.
      const header = fs.readFileSync(remoteHelper).subarray(0, 4);
      assert.ok(platform === 'win32' ? header.subarray(0, 2).toString() === 'MZ' : header.toString('hex') === '7f454c46', 'remote-access helper target header');
      const remote = path.join(stage, 'native', platform === 'win32' ? 'fb-remote.exe' : 'fb-remote');
      fs.copyFileSync(remoteHelper, remote);
      fs.chmodSync(remote, 0o755);
    }
    fs.writeFileSync(path.join(stage, 'package.json'), JSON.stringify(metadata, null, 2) + '\n');
    await auditInput(stage, platform);
    return await packager({
      dir: stage, out: options.out || path.join(root, 'electron/out'),
      platform, arch, appVersion: metadata.version,
      name: 'FreedomBooks', executableName: platform === 'linux' ? 'freedombooks' : 'FreedomBooks', appBundleId: 'app.freedombooks.FreedomBooks',
      // ASAR matches absolute filenames with matchBase:true. A basename glob
      // also selects native modules below hidden CI temporary directories.
      electronVersion: '45.2.0', asar: {unpack: NATIVE_UNPACK}, overwrite: true,
      icon: platform === 'win32' ? path.join(__dirname, 'windows/FreedomBooks.ico') : undefined,
      afterComplete: [async ({buildPath}) => {
        const binary = path.join(buildPath, platform === 'win32' ? 'FreedomBooks.exe' : 'freedombooks');
        installNotices(buildPath);
        if (platform === 'linux') {
          // Packager's temporary root is 0700. Installed program files must be
          // traversable by ordinary users and must never be world-writable.
          const normalize = directory => {
            fs.chmodSync(directory, 0o755);
            for (const entry of fs.readdirSync(directory, {withFileTypes: true})) {
              const file = path.join(directory, entry.name);
              if (entry.isDirectory()) normalize(file);
              else if (entry.isFile()) fs.chmodSync(file, (fs.statSync(file).mode & 0o111) ? 0o755 : 0o644);
            }
          };
          normalize(buildPath);
        }
        if (incomplete) fs.writeFileSync(path.join(buildPath, 'INCOMPLETE-WINDOWS-NATIVE.txt'), metadata.freedomBooksIncomplete + '\nSee packaging/WINDOWS-BLOCKERS.md in the source tree.\n');
        await harden(binary);
        console.log(`FUSES VERIFIED ${platform}/${arch} ${binary}`);
      }],
    });
  } finally { fs.rmSync(stage, {recursive: true, force: true}); }
}
function cliOptions(env = process.env) {
  return env.FB_SIMPLEFIN === undefined ? {} : {simplefin: env.FB_SIMPLEFIN === '1'};
}
if (require.main === module) build(cliOptions()).then(builds => builds.forEach(b => console.log(`PACKAGED ${b}`))).catch(e => {console.error(e); process.exitCode = 1;});
module.exports = {build, NATIVE_UNPACK, cliOptions};
