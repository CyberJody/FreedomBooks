'use strict';
// Synthetic Rust-rendered HTML + real sandboxed Electron/Chromium controls, no native book or network.
// Generate fixtures with bankfeed_review_workflow, then run this under the shared runner and Xvfb.
const path = require('node:path');
const fs = require('node:fs');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
const fixtures = process.env.FB_REVIEW_ARTIFACTS;
assert.ok(fixtures && path.isAbsolute(fixtures), 'FB_REVIEW_ARTIFACTS must name synthetic Rust render output');

if (process.versions.electron && !process.env.ELECTRON_RUN_AS_NODE) {
  const {app, BrowserWindow, protocol} = require('electron');
  const {CSP, staticFile} = require('../main/guards.cjs');
  protocol.registerSchemesAsPrivileged([{scheme:'fb', privileges:{standard:true, secure:true, supportFetchAPI:true}}]);
  app.whenReady().then(() => {
    protocol.handle('fb', request => {
      const url = new URL(request.url);
      if (url.pathname.startsWith('/static/')) return staticFile(path.join(root, 'renderer/static'), url.pathname.slice('/static/'.length));
      const file = url.pathname.includes('/rules/new/') ? 'rule.html'
        : url.pathname === '/failed' ? 'failed.html' : url.pathname === '/matching' ? 'matching.html' : 'initial.html';
      return new Response(fs.readFileSync(path.join(fixtures, file)), {headers:{'content-type':'text/html; charset=utf-8', 'content-security-policy':CSP}});
    });
    const win = new BrowserWindow({width:1536, height:1080, webPreferences:{sandbox:true, contextIsolation:true, nodeIntegration:false}});
    win.loadURL('fb://app/initial');
  });
  app.on('window-all-closed', () => app.quit());
} else {
  const {_electron: electron} = require('playwright');
  async function main() {
    const data = fs.mkdtempSync(path.join(process.env.TMPDIR || '/tmp', 'fb-review-render-'));
    const app = await electron.launch({chromiumSandbox:true, args:[__filename], cwd:root, env:{...process.env, HOME:path.join(data,'home'), XDG_CONFIG_HOME:path.join(data,'config'), XDG_DATA_HOME:path.join(data,'data'), XDG_CACHE_HOME:path.join(data,'cache') }});
    const page = await app.firstWindow();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.setDefaultTimeout(10000);
    await page.setViewportSize({width:1536, height:1080});
    const ids = JSON.parse(fs.readFileSync(path.join(fixtures,'fixture.json'),'utf8'));
    const row = name => page.locator(`[data-review-row="${ids.ids[name]}"]`);
    const pick = name => row(name).locator('input[name=sel]');
    const field = (name, key) => row(name).locator(`[name="${key}_${ids.ids[name]}"]`);
    const shot = name => page.screenshot({path:path.join(fixtures, name+'.png'), fullPage:false});
    let passes = 0;
    const pass = message => console.log(`PASS ${++passes}: ${message}`);
    const ready = () => page.waitForSelector('#review-tools:not([hidden])');
    try {
      await ready();
      assert.equal(await page.locator('input[name=sel]').count(), 7);
      assert.equal(await page.isDisabled('#bulk-apply'), true);
      // Arrow and range navigation stays on row checkboxes.
      const picks = page.locator('input[name=sel]');
      await picks.nth(0).focus();
      await page.keyboard.press('Shift+ArrowDown');
      assert.equal(await page.locator('input[name=sel]:checked').count(), 2);
      assert.equal(await picks.nth(1).evaluate(el => el === document.activeElement), true);
      await page.keyboard.press('ArrowDown');
      assert.equal(await picks.nth(2).evaluate(el => el === document.activeElement), true);
      await page.keyboard.press('Space');
      assert.equal(await picks.nth(2).isChecked(), true);
      await page.keyboard.press('Control+a');
      assert.equal(await page.locator('input[name=sel]:checked').count(), 7);
      await page.click('#review-clear');
      await picks.nth(0).click();
      await picks.nth(3).click({modifiers:['Shift']});
      assert.equal(await page.locator('input[name=sel]:checked').count(), 4);
      pass('keyboard movement, Shift range, Space, Ctrl+A and Shift+click select only this page');

      await page.click('#review-clear');
      await field('OFFICE debit','payee').fill('Keep my text');
      await field('OFFICE debit','payee').focus();
      await page.keyboard.press('Control+a');
      assert.equal(await field('OFFICE debit','payee').evaluate(el => el.selectionEnd - el.selectionStart), 12);
      await page.keyboard.press('ArrowDown');
      assert.equal(await field('OFFICE debit','payee').evaluate(el => el === document.activeElement), true);
      assert.equal(await page.locator('input[name=sel]:checked').count(), 0);
      await field('OFFICE debit','kind').focus();
      await page.keyboard.press('ArrowDown');
      assert.equal(await field('OFFICE debit','kind').evaluate(el => el === document.activeElement), true);
      assert.equal(await page.locator('input[name=sel]:checked').count(), 0);
      pass('text selection and native select arrows keep their normal shortcuts');

      await page.check('#review-all');
      await page.selectOption('#bulk-category', ids.expense);
      await page.check('#bulk-set-payee');
      await page.fill('#bulk-payee','Group vendor');
      assert.match(await page.textContent('#bulk-summary'), /5 drafts will change; 2 skipped; 7 selected/);
      assert.match(await page.textContent('#bulk-changes'), /Still pending/);
      assert.match(await page.textContent('#bulk-changes'), /Map to an active/);
      await shot('01-bulk-preview');
      await page.evaluate(() => {
        window.reviewSubmits = 0;
        document.getElementById('bankfeed-review').addEventListener('submit', e => {e.preventDefault();window.reviewSubmits++;});
      });
      await page.locator('#bulk-payee').press('Enter');
      assert.equal(await page.evaluate(() => window.reviewSubmits), 0);
      await page.click('#bulk-apply');
      assert.equal(await page.evaluate(() => window.reviewSubmits), 0);
      assert.match(await page.textContent('#bulk-result'), /5 review drafts updated; 2 skipped. Nothing posted/);
      for (const name of ['OFFICE debit','OFFICE refund','OFFICE card charge','OFFICE card refund','OTHER transfer']) {
        assert.equal(await field(name,'account').inputValue(), ids.expense);
        assert.equal(await field(name,'payee').inputValue(), 'Group vendor');
        assert.equal(await field(name,'kind').inputValue(), name==='OFFICE debit' || name==='OTHER transfer' ? 'CHECK' : 'JOURNAL');
      }
      assert.equal(await field('OFFICE unmapped','payee').inputValue(), '');
      assert.equal(await field('OFFICE pending','payee').inputValue(), '');
      pass('preview and apply handle mixed bank/card directions, skip pending/unmapped and never submit');

      await page.selectOption('#bulk-rule',ids.rule);
      assert.match(await page.textContent('#bulk-summary'), /2 drafts will change; 5 skipped; 7 selected/);
      await page.click('#bulk-apply');
      assert.equal(await field('OFFICE debit','payee').inputValue(),'Office vendor');
      assert.equal(await field('OFFICE card charge','payee').inputValue(),'Office vendor');
      assert.equal(await field('OFFICE refund','payee').inputValue(),'Group vendor');
      assert.equal(await field('OTHER transfer','payee').inputValue(),'Group vendor');
      pass('reusable rule follows Rust text/sign/amount matches and leaves other selected rows intact');

      await page.selectOption('#bulk-rule','');
      await page.selectOption('#bulk-category',ids.bank);
      assert.match(await page.textContent('#bulk-changes'), /mapped account itself/);
      await page.click('#review-clear');
      assert.equal(await page.isDisabled('#bulk-apply'),true);
      pass('mapped-account conflicts and empty selection cannot apply unintended changes');
      // Filters prompt before losing edits, then reset the page-scoped selection.
      page.once('dialog', dialog => dialog.dismiss());
      await page.getByRole('button',{name:'Show',exact:true}).click();
      assert.equal(new URL(page.url()).pathname,'/initial');
      page.once('dialog', dialog => dialog.accept());
      await page.getByRole('button',{name:'Show',exact:true}).click();
      await page.waitForURL(/\?status=/); await ready();
      assert.equal(await page.locator('input[name=sel]:checked').count(),0);
      pass('filter changes warn before discarding edited drafts and clear selection');

      await field('OFFICE debit','payee').fill('Reviewed payee');
      await field('OFFICE debit','account').selectOption(ids.expense);
      page.once('dialog', dialog => dialog.accept());
      await row('OFFICE debit').locator('[data-create-rule]').click();
      await page.waitForURL(/\/rules\/new\//);
      assert.equal(new URL(page.url()).searchParams.get('payee'),'Reviewed payee');
      assert.equal(new URL(page.url()).searchParams.get('category'),ids.expense);
      assert.equal(await page.inputValue('#id_payee_name'),'Reviewed payee');
      assert.equal(await page.inputValue('#id_account'),ids.expense);
      await shot('02-create-rule');
      pass('Create rule carries reviewed category and payee into the existing editable rule form');

      await page.goto('fb://app/failed');await ready();
      const failed = page.locator('.row-error');
      assert.equal(await failed.locator('input[name=sel]').isChecked(),true);
      assert.equal(await failed.locator('input[name=sel]').evaluate(el => el === document.activeElement),true);
      assert.match(await failed.locator('.row-error-text').textContent(), /Map /);
      assert.equal(await field('OFFICE card charge','payee').inputValue(),'Edited OFFICE card charge');
      assert.equal(await page.inputValue('#id_q'),'OFFICE');
      page.once('dialog', dialog => dialog.dismiss());
      await page.getByRole('button',{name:'Show',exact:true}).click();
      assert.equal(new URL(page.url()).pathname,'/failed');
      await shot('03-partial-failure');
      pass('real Rust partial-failure HTML retains row edits/selection, filters and accessible error focus');
      await page.goto('fb://app/matching');await ready();
      const matching = page.locator('tr[data-review-row]').filter({has:page.locator('.desc', {hasText:'OFFICE debit'})});
      assert.equal(await matching.locator('select[name^=kind_]').inputValue(),'MATCH');
      const matchLine = await matching.locator('select[name^=line_]').inputValue();
      await matching.locator('input[name=sel]').check();
      assert.equal(await page.inputValue('#bulk-kind'),'AUTO');
      await page.selectOption('#bulk-category',{label:'500 · Office supplies'});
      assert.equal(await page.isDisabled('#bulk-apply'),true);
      assert.equal(await matching.locator('select[name^=line_]').inputValue(),matchLine);
      assert.equal(await matching.locator('select[name^=kind_]').inputValue(),'MATCH');
      await shot('04-duplicate-match');
      pass('default bulk categorization preserves duplicate-match choice');
      assert.deepEqual(errors,[]);
      console.log(`Synthetic UI fixture and screenshots retained at ${fixtures}`);
    } catch (error) {
      fs.writeFileSync(path.join(fixtures,'ui-error.html'),await page.content());
      await shot('ui-error');
      throw error;
    } finally { await app.close(); }
  }
  main().catch(error => {console.error(error);process.exitCode=1;});
}
