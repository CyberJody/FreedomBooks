'use strict';
// What "Report a problem" shows for copying into an issue (DESKTOP-JODY1): the operating system's name and
// version, nothing else about the machine (no host name, user, hardware or paths). The app version comes
// from Electron's app.getVersion(), i.e. the packaged build's own metadata.
const fs = require('node:fs');

/** PRETTY_NAME (or NAME VERSION_ID) from os-release text; '' if absent. */
function osRelease(text) {
  const values = {};
  for (const line of String(text || '').split('\n')) {
    const m = /^([A-Z_]+)=(.*)$/.exec(line.trim());
    if (m) values[m[1]] = m[2].replace(/^(["'])(.*)\1$/, '$2');
  }
  return values.PRETTY_NAME || [values.NAME, values.VERSION_ID].filter(Boolean).join(' ');
}

function read(file) {
  try { return fs.readFileSync(file, 'utf8'); } catch { return ''; }
}

/** e.g. "Debian GNU/Linux 13 (trixie)", "macOS 15.1", "Windows 10.0.26100". */
function osLabel({ platform = process.platform, systemVersion = process.getSystemVersion ? process.getSystemVersion() : '', readFile = read } = {}) {
  if (platform === 'darwin') return `macOS ${systemVersion}`.trim();
  if (platform === 'win32') return `Windows ${systemVersion}`.trim();
  if (platform === 'linux') {
    // Inside a Flatpak, /etc/os-release is the runtime's; the host's copy is under /run/host.
    for (const file of ['/run/host/os-release', '/etc/os-release', '/usr/lib/os-release']) {
      const name = osRelease(readFile(file));
      if (name) return name;
    }
    return 'Linux';
  }
  return String(platform);
}

module.exports = { osRelease, osLabel };
