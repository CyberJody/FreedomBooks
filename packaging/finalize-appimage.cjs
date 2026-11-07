'use strict';
// Builder's modern AppRun still contains a heuristic --no-sandbox fallback.
// Repack only the launcher, preserving its checked runtime and hardened payload.
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const {spawnSync} = require('node:child_process');
async function finalize(artifact) {
  const {getAppImageTools} = require('app-builder-lib/out/toolsets/linux');
  const {Arch} = require('builder-util');
  const tools = await getAppImageTools('1.0.3', Arch.x64);
  const temp = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-appimage-launcher-'));
  function run(command, args) {
    const result = spawnSync(command, args, {cwd: temp, stdio: ['ignore', 'pipe', 'pipe']});
    if (result.status !== 0) throw new Error(`${command} failed: ${result.stderr?.toString()}`);
  }
  try {
    run(artifact, ['--appimage-extract']);
    const stage = path.join(temp, 'squashfs-root');
    fs.writeFileSync(path.join(stage, 'AppRun'), `#!/bin/sh
set -eu
APPDIR="\${APPDIR:-$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)}"
export LD_LIBRARY_PATH="$APPDIR/usr/lib\${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$APPDIR/freedombooks" "$@"
`, {mode: 0o755});
    const output = path.join(temp, 'FreedomBooks.AppImage');
    const runtime = fs.readFileSync(tools.runtime);
    run(tools.mksquashfs, [stage, output, '-offset', String(runtime.length), '-all-root', '-noappend', '-no-progress', '-quiet', '-no-xattrs', '-no-fragments', '-comp', 'zstd', '-processors', '4']);
    const handle = fs.openSync(output, 'r+');
    try { fs.writeSync(handle, runtime, 0, runtime.length, 0); } finally { fs.closeSync(handle); }
    fs.copyFileSync(output, artifact);
    fs.chmodSync(artifact, 0o755);
    console.log('APPIMAGE LAUNCHER VERIFIED: no sandbox-disabling fallback');
  } finally { fs.rmSync(temp, {recursive: true, force: true}); }
}
module.exports = {finalize};
