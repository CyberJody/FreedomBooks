// --prepackaged preserves packager's ASAR, embedded integrity and fuse wire.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const desktopText = fs.readFileSync(path.join(__dirname, 'flatpak/app.freedombooks.FreedomBooks.desktop'), 'utf8');
const desktop = Object.fromEntries(desktopText.split('\n').filter(line => line.includes('=')).map(line => [line.slice(0, line.indexOf('=')), line.slice(line.indexOf('=') + 1)]));
delete desktop.Exec;
// Builder installs icons by executable name; use that name in generated entries.
delete desktop.Icon;
module.exports = {
  appId: 'app.freedombooks.FreedomBooks', productName: 'FreedomBooks',
  executableName: 'FreedomBooks', electronVersion: '45.2.0',
  directories: {app: path.join(root, 'electron'), output: path.join(__dirname, 'dist')},
  toolsets: {appimage: '1.0.3', wine: '1.0.1'},
  npmRebuild: false, nodeGypRebuild: false, asar: true,
  publish: null,
  linux: {
    target: ['AppImage', 'deb'],
    category: 'Office;Finance', synopsis: desktop.Comment,
    description: 'FreedomBooks accounting for small and medium businesses.',
    maintainer: 'FreedomBooks <releases@freedombooks.app>',
    icon: path.join(__dirname, 'flatpak/icons/512x512/app.freedombooks.FreedomBooks.png'),
    executableName: 'freedombooks',
    syncDesktopName: true, desktop: {entry: desktop},
    artifactName: 'FreedomBooks-${version}-${arch}.${ext}',
  },
  deb: {
    // Names shared by Debian 13 and current Linux Mint. apt selects t64 replacements where needed.
    depends: ['libgtk-3-0', 'libnss3', 'libnspr4', 'libasound2t64 | libasound2', 'libatk1.0-0',
      'libatk-bridge2.0-0', 'libatspi2.0-0', 'libcups2', 'libdrm2', 'libgbm1', 'libx11-6',
      'libxcb1', 'libxcomposite1', 'libxdamage1', 'libxext6', 'libxfixes3', 'libxrandr2',
      'libxkbcommon0', 'libpango-1.0-0', 'libcairo2', 'libglib2.0-0', 'libc6 (>= 2.38)',
      'libstdc++6 (>= 13)', 'libgcc-s1', 'ca-certificates', 'xdg-utils', 'apparmor'],
    afterInstall: path.join(__dirname, 'linux/after-install.sh'),
    afterRemove: path.join(__dirname, 'linux/after-remove.sh'),
  },
  appImage: {artifactName: 'FreedomBooks-${version}-${arch}.AppImage'},
  win: {target: ['nsis'], icon: path.join(__dirname, 'windows/FreedomBooks.ico'),
    // packager already edited the PE and set integrity + fuses; don't edit it again.
    signAndEditExecutable: false},
  nsis: {oneClick: true, perMachine: false, allowElevation: false, packElevateHelper: false,
    createDesktopShortcut: true, createStartMenuShortcut: true, shortcutName: 'FreedomBooks',
    deleteAppDataOnUninstall: false, artifactName: 'FreedomBooks-Setup-${version}.exe'},
};
