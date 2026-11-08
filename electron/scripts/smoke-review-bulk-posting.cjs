'use strict';
// Runs while the Rust fixture remains open. Return actual renderer FormData to that same App.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
const dir = process.argv[2];
if (process.versions.electron && !process.env.ELECTRON_RUN_AS_NODE) {
  const {app, BrowserWindow, protocol} = require('electron');
  const {CSP, staticFile} = require('../main/guards.cjs');
  protocol.registerSchemesAsPrivileged([{scheme:'fb', privileges:{standard:true, secure:true, supportFetchAPI:true}}]);
  app.whenReady().then(() => {
    protocol.handle('fb', request => {
      const url = new URL(request.url);
      if(url.pathname.startsWith('/static/')) return staticFile(path.join(root,'renderer/static'),url.pathname.slice(8));
      return new Response(fs.readFileSync(path.join(dir,'review.html')), {headers:{'content-type':'text/html; charset=utf-8','content-security-policy':CSP}});
    });
    const win = new BrowserWindow({width:1536,height:1080,webPreferences:{sandbox:true,contextIsolation:true,nodeIntegration:false}});
    win.loadURL('fb://app/review');
  });
  app.on('window-all-closed',()=>app.quit());
} else {
  async function main() {
    const {_electron} = require('playwright');
    const data=fs.mkdtempSync(path.join(process.env.TMPDIR || '/tmp','fb-bulk-'));
    const app=await _electron.launch({chromiumSandbox:true,args:[__filename,dir],cwd:root,env:{...process.env,
      HOME:path.join(data,'home'),XDG_CONFIG_HOME:path.join(data,'config'),XDG_DATA_HOME:path.join(data,'data'),XDG_CACHE_HOME:path.join(data,'cache')}});
    try {
      const page=await app.firstWindow();
      page.setDefaultTimeout(10000);
      await page.waitForSelector('#review-tools:not([hidden])');
      const ids=JSON.parse(fs.readFileSync(path.join(dir,'input.json')));
      const row=id=>page.locator('[data-review-row="'+id+'"]');
      const field=(id,key)=>row(id).locator('[name="'+key+'_'+id+'"]');
      assert.equal(await field(ids.match,'kind').inputValue(),'MATCH');
      assert.equal(await field(ids.receipt,'kind').inputValue(),'DEPOSIT');
      await field(ids.transfer,'kind').selectOption('TRANSFER');
      await field(ids.transfer,'to').selectOption(ids.savings);
      const original={};
      for(const id of [ids.match,ids.receipt,ids.transfer]) {
        original[id]=await row(id).locator('input:not([name=sel]),select').evaluateAll(es=>es.map(e=>[e.name,e.value]));
      }
      for(const id of [ids.match,ids.receipt,ids.transfer,ids.ordinary]) await row(id).locator('[name=sel]').check();
      assert.equal(await page.inputValue('#bulk-kind'),'AUTO');
      await page.selectOption('#bulk-category',ids.expense);
      await page.check('#bulk-set-payee');
      await page.fill('#bulk-payee','Bulk reviewed payee');
      for(const mode of ['AUTO','CHECK','JOURNAL','KEEP']) {
        await page.selectOption('#bulk-kind',mode);
        assert.match(await page.textContent('#bulk-summary'),/3 skipped|4 skipped/,'special classifications must be skipped in every bulk mode');
      }
      await page.selectOption('#bulk-kind','AUTO');
      await page.click('#bulk-apply');
      for(const id of [ids.match,ids.receipt,ids.transfer]) {
        assert.deepEqual(await row(id).locator('input:not([name=sel]),select').evaluateAll(es=>es.map(e=>[e.name,e.value])),original[id]);
      }
      assert.equal(await field(ids.ordinary,'payee').inputValue(),'Bulk reviewed payee');
      await page.evaluate(()=>{
        document.getElementById('bankfeed-review').addEventListener('submit',e=>{
          e.preventDefault();
          globalThis.reviewSubmission=Array.from(new FormData(e.target).entries());
        });
      });
      await page.getByRole('button',{name:'Accept selected',exact:true}).click();
      const fields=await page.evaluate(()=>globalThis.reviewSubmission);
      assert.ok(fields?.length);
      fs.writeFileSync(path.join(dir,'submission.json'),JSON.stringify(fields));
      await page.screenshot({path:path.join(dir,'preserved-classifications.png')});
      console.log('PASS renderer preserves MATCH/DEPOSIT/TRANSFER in every bulk mode and returns actual Accept fields');
    } finally { await app.close(); }
  }
  main().catch(e=>{console.error(e);process.exitCode=1;});
}
