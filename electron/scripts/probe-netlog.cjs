'use strict';
// Launch the app with Chromium's net log, wait on the first screen, quit, and check the log
// (smoke-release-net.cjs netlog). A quick probe for start-up traffic; the full check is the smoke.
const { _electron: electron } = require('playwright');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const file = process.env.FB_SMOKE_NETLOG;
const userData = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-probe-'));
(async () => {
  const app = await electron.launch({ args: [`--log-net-log=${file}`, root], cwd: root, timeout: 60000,
    env: { ...process.env, FB_DESKTOP_USER_DATA: userData, XDG_CONFIG_HOME: path.join(userData, 'config') } });
  const page = await app.firstWindow();
  await page.waitForSelector('main h1');
  await page.waitForTimeout(Number(process.env.FB_PROBE_WAIT || 20000));
  await app.close();
  fs.rmSync(userData, { recursive: true, force: true });
  const events = require('./smoke-release-net.cjs').netlog(file);
  console.log(`PROBE OK: ${events} net log events, nothing outside fb://app/`);
})().catch((e) => { console.error(`PROBE FAILED: ${e.stack || e}`); process.exit(1); });
