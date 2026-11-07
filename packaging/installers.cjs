// Locally build an unsigned installer, never publish. Signing belongs after hardening.
'use strict';
const path = require('node:path');
const fs = require('node:fs');
const {build: packageApp} = require('./package.cjs');
const {verify} = require('./fuses.cjs');
const config = require('./builder.cjs');
async function main() {
  const platform = process.argv[2] || process.platform;
  if (!['linux', 'win32'].includes(platform)) throw new Error('usage: node installers.cjs linux|win32');
  const version = process.env.FB_RELEASE_VERSION || require('../electron/package.json').version;
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) throw new Error('Invalid release version');
  const root = path.resolve(__dirname, '..');
  const native = process.env.FB_INSTALLER_NATIVE || path.join(root, platform === 'win32' ? 'electron/native/win32-x64/fb-node.node' : 'electron/native/fb-node.node');
  const packaged = process.argv.includes('--wrap-only') ? path.join(root, `electron/out/FreedomBooks-${platform}-x64`)
    : (await packageApp({platform, native, version}))[0];
  const binary = path.join(packaged, platform === 'win32' ? 'FreedomBooks.exe' : 'freedombooks');
  await verify(binary);
  // A two-package builder project reads metadata only from this clean copy of
  // the ASAR's package.json, not the source Electron development package.
  const {extractFile} = await import('@electron/asar');
  const metadata = JSON.parse(extractFile(path.join(packaged, 'resources/app.asar'), 'package.json'));
  if (metadata.freedomBooksIncomplete || !fs.existsSync(path.join(packaged, 'resources/app.asar.unpacked/native/fb-node.node'))) {
    throw new Error('Installer blocked: Windows native module is missing; never release the incomplete developer shell');
  }
  if (metadata.version !== version) throw new Error('Repackage before changing the release version');
  const metadataDir = path.join(__dirname, '.work/metadata', platform);
  fs.mkdirSync(metadataDir, {recursive: true});
  fs.writeFileSync(path.join(metadataDir, 'package.json'), JSON.stringify({...metadata, author: {name: 'FreedomBooks'}, homepage: 'https://freedombooks.app'}));
  const {build, Platform, Arch} = require('electron-builder');
  await build({projectDir: __dirname, prepackaged: packaged, publish: 'never',
    targets: (platform === 'linux' ? Platform.LINUX : Platform.WINDOWS).createTarget(platform === 'linux' ? ['AppImage', 'deb'] : ['nsis'], Arch.x64),
    config: {...config, directories: {...config.directories, app: metadataDir}}});
  if (platform === 'linux') {
    const artifact = path.join(__dirname, 'dist', `FreedomBooks-${version}-x86_64.AppImage`);
    await require('./finalize-appimage.cjs').finalize(artifact);
  }
  await verify(path.join(packaged, platform === 'win32' ? 'FreedomBooks.exe' : 'freedombooks'));
  console.log(`INSTALLERS BUILT ${platform}/x64 ${version}; FUSES VERIFIED after wrapping; output ${path.join(__dirname, 'dist')}`);
}
main().catch(error => {console.error(error); process.exitCode = 1;});
