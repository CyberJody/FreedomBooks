'use strict';
// DESKTOP-JODY1: open "Report a problem" from the footer, check the how-to and the copyable version/OS,
// and click the issue link with shell.openExternal stubbed in the main process (no real browser opens).
const assert = require('node:assert/strict');
const path = require('node:path');
const ISSUE = 'https://github.com/CyberJody/FreedomBooks/issues/new/choose';

module.exports = async ({ app, page, shots, shot, pass, heading }) => {
  const stub = await app.evaluate(({ shell }) => {
    const opened = [];
    const fake = async (url) => { opened.push(url); };
    fake.fbSmokeStub = true;
    shell.openExternal = fake;
    globalThis.__fbOpened = opened;
    return shell.openExternal.fbSmokeStub === true;
  });
  assert.equal(stub, true, 'shell.openExternal must be stubbed before any click');
  const opened = () => app.evaluate(() => globalThis.__fbOpened.slice());

  const footer = (await page.locator('footer').textContent()).trim();
  assert.doesNotMatch(footer, /payroll/i);
  // Release builds hide the plugins, and with them the footer's bank-feed phrase.
  const plugins = await app.evaluate(({ app: a }) => {
    const require = process.getBuiltinModule('module').createRequire(a.getAppPath() + '/package.json');
    return require(require('node:path').join(a.getAppPath(), 'native', 'fb-node.node')).pluginsEnabled();
  });
  assert.equal(footer, `FreedomBooks · ${footer.split(' · ')[1]}${plugins ? ' · Bank feeds are read-only' : ''} · Report a problem`);
  await Promise.all([page.waitForURL('fb://app/help/report/'), page.click('footer a.report-problem')]);
  assert.equal(await heading(page), 'Report a problem');
  const expected = await app.evaluate(({ app: a }) => {
    const require = process.getBuiltinModule('module').createRequire(a.getAppPath() + '/package.json');
    return { version: a.getVersion(), os: require(require('node:path').join(a.getAppPath(), 'main', 'about.cjs')).osLabel() };
  });
  const main = await page.locator('main').innerText();
  assert.equal(await page.locator('pre.report-details').innerText(), `FreedomBooks version: ${expected.version}\nOperating system: ${expected.os}`);
  for (const text of ['GitHub issues are public.', 'Never paste real financial data, customer names, passwords, keys or bank details.',
    'Write the steps you took', 'Say what you expected to happen.', 'Say what actually happened.',
    'Attach a screenshot only after covering names, amounts and account numbers.'])
    assert.ok(main.includes(text), text);
  assert.doesNotMatch(main, /No report address|person who runs this copy|outside address|mailto|e-?mail/i);
  assert.equal(await page.locator('main code').innerText(), ISSUE);
  assert.doesNotMatch((await page.locator('footer').textContent()), /payroll/i);
  await shot(page, 'jody1-report-problem');
  await page.locator('footer').screenshot({ path: path.join(shots, 'jody1-footer.png') });
  console.log(`      screenshot ${path.join(shots, 'jody1-footer.png')}`);

  const before = page.url();
  await page.click('a.report-issue-link');
  for (let i = 0; i < 50 && (await opened()).length === 0; i++) await page.waitForTimeout(100);
  assert.deepEqual(await opened(), [ISSUE]);
  assert.equal(page.url(), before, 'the window never navigates away');
  // Any other outside link in the page does nothing at all.
  await page.evaluate(() => {
    for (const href of ['https://github.com/CyberJody/Other/issues/new/choose', 'http://github.com/CyberJody/FreedomBooks/issues/new/choose', 'https://example.com/']) {
      const a = document.createElement('a'); a.href = href; a.textContent = href; a.className = 'smoke-other'; document.querySelector('main').append(a);
    }
  });
  for (const link of await page.locator('a.smoke-other').all()) await link.click();
  await page.waitForTimeout(300);
  assert.equal(page.url(), before);
  // The real IPC handler, with the real window sender, refuses tampered URLs.
  const refused = await app.evaluate(async ({ ipcMain, BrowserWindow }) => {
    const contents = BrowserWindow.getAllWindows()[0].webContents;
    const handler = ipcMain._invokeHandlers.get('fb:app.openExternal');
    const out = [];
    for (const url of ['https://github.com/CyberJody/FreedomBooks/issues/new/choose?x=1', 'http://github.com/CyberJody/FreedomBooks/issues/new/choose', 'https://github.com/Other/FreedomBooks/issues/new/choose', 'https://example.com/']) {
      const r = await handler({ sender: contents, senderFrame: contents.mainFrame }, { url });
      out.push(!r.ok && /link not allowed/.test(r.error));
    }
    return out;
  });
  assert.deepEqual(refused, [true, true, true, true]);
  assert.deepEqual(await opened(), [ISSUE], 'only the exact issue URL reached shell.openExternal');
  pass('report-a-problem how-to from the footer: version/OS shown, no payroll footer text, issue URL opened via the main process only');
};
