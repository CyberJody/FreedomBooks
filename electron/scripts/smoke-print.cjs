'use strict';
// Called from the real app smoke after posting its invoice, before subsequent scenario mutations.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {spawnSync} = require('node:child_process');

module.exports = async ({app,page,company,invoiceId,shot,pass}) => {
  const output=process.env.FB_SMOKE_SHOTS;
  const original=page.url();
  const evidence=[];
  await app.evaluate(({Menu,dialog,BrowserWindow})=>{
    globalThis.__printSmoke={popup:Menu.prototype.popup,save:dialog.showSaveDialog,calls:[],action:'pdf'};
    const w=BrowserWindow.getAllWindows()[0];
    globalThis.__printSmoke.pdf=w.webContents.printToPDF;
    globalThis.__printSmoke.print=w.webContents.print;
    Menu.prototype.popup=function(options){
      const item=this.items.find(i=>i.label===(globalThis.__printSmoke.action==='pdf'?'Save as PDF…':'Print…'));
      item.click();
      options.callback();
    };
    dialog.showSaveDialog=async (_window,options)=>{
      globalThis.__printSmoke.calls.push({kind:'save-dialog',process:process.type,options});
      return globalThis.__printSmoke.cancel?{canceled:true}:{canceled:false,filePath:globalThis.__printSmoke.file};
    };
    w.webContents.printToPDF=function(options){
      globalThis.__printSmoke.calls.push({kind:'pdf',process:process.type,options});
      return globalThis.__printSmoke.pdf.call(this,options);
    };
    w.webContents.print=function(options,callback){
      globalThis.__printSmoke.calls.push({kind:'print',process:process.type,options});
      callback(true);
    };
  });
  const save=async(name,checks)=>{
    const file=path.join(output,name+'.pdf');fs.rmSync(file,{force:true});
    await app.evaluate((_electron,file)=>{globalThis.__printSmoke.file=file;globalThis.__printSmoke.cancel=false;globalThis.__printSmoke.action='pdf';},file);
    await page.click('#print-now');
    await page.waitForFunction(()=>!document.querySelector('#print-now').disabled);
    assert.ok(fs.existsSync(file),'native Save dialog path must receive the PDF');
    assert.equal(fs.readFileSync(file).subarray(0,5).toString(),'%PDF-');
    assert.equal(fs.statSync(file).mode&0o777,0o600);
    const text=spawnSync('pdftotext',['-layout',file,'-'],{encoding:'utf8'});assert.equal(text.status,0,text.stderr);
    fs.writeFileSync(path.join(output,name+'.txt'),text.stdout);
    for(const pattern of checks)assert.match(text.stdout,pattern);
    assert.doesNotMatch(text.stdout,/Print \/ Save as PDF|Back to (?:invoice|the report)/);
    const info=spawnSync('pdfinfo',[file],{encoding:'utf8'});assert.equal(info.status,0,info.stderr);
    assert.match(info.stdout,/Page size:\s+612 x 792 pts/);
    fs.writeFileSync(path.join(output,name+'-pdfinfo.txt'),info.stdout);
    const render=spawnSync('pdftoppm',['-f','1','-singlefile','-scale-to','1440','-png',file,path.join(output,name+'-page1')],{encoding:'utf8'});
    assert.equal(render.status,0,render.stderr);
    evidence.push({file,checks:checks.map(String),opens:true,textExtraction:true,mode:'0600'});
    pass(`PRINT ${name}: native Save dialog -> main-process printToPDF -> PDF opens and text/totals verified`);
  };
  try {
    await page.goto(`fb://app/c/${company}/invoices/${invoiceId}/`);
    const windows=(await app.windows()).length;
    await page.getByRole('link',{name:'Print / Save as PDF',exact:true}).first().click();
    await page.waitForURL(/\/invoices\/[0-9a-f-]+\/print\/$/);
    assert.equal((await app.windows()).length,windows);
    assert.equal(await page.textContent('#print-total'),'180.00');
    assert.equal(await page.textContent('#print-balance'),'180.00');
    await shot(page,'print-live-invoice');
    assert.ok(await page.evaluate(()=>document.adoptedStyleSheets.some(s=>Array.from(s.cssRules).some(r=>r.cssText.startsWith('@page')))),'strict-CSP footer must be restored through CSSOM');
    await save('invoice',['INV-000001','Total USD','Balance due','180.00','revision 2 of 2','Page 1 of 1'].map(s=>new RegExp(s.replace(/[.*+?^${}()|[\]\\]/g,'\\$&'))));
    // Reuse the button after cancellation and after printing: capabilities are single-use and renewed.
    const cancelled=path.join(output,'cancelled.pdf');fs.rmSync(cancelled,{force:true});
    await app.evaluate((_e,file)=>{globalThis.__printSmoke.cancel=true;globalThis.__printSmoke.file=file;},cancelled);
    await page.click('#print-now');await page.waitForFunction(()=>!document.querySelector('#print-now').disabled);
    assert.equal(fs.existsSync(cancelled),false);
    await app.evaluate(()=>{globalThis.__printSmoke.action='print';});
    await page.click('#print-now');await page.waitForFunction(()=>!document.querySelector('#print-now').disabled);
    pass('PRINT cancelled save leaves no file; renewed button invokes main-process native print');
    await page.goto(`fb://app/c/${company}/reports/profit-and-loss/?preset=custom&start=2026-10-01&end=2026-10-31`);
    await page.getByRole('link',{name:'Print',exact:true}).click();
    await page.waitForURL(/output=print/);
    await shot(page,'print-live-profit-loss');
    await save('profit-loss',[/Profit and loss/,/Lawn care income/,/Net income/,/180\.00/,/Page 1 of 1/]);
    const calls=await app.evaluate(()=>globalThis.__printSmoke.calls);
    assert.equal(calls.filter(c=>c.kind==='pdf').length,2);
    assert.equal(calls.filter(c=>c.kind==='print').length,1);
    assert.ok(calls.every(c=>c.process==='browser'));
    assert.ok(calls.filter(c=>c.kind==='save-dialog').every(c=>c.options.filters[0].extensions[0]==='pdf'));
    assert.ok(calls.filter(c=>c.kind==='pdf').every(c=>c.options.preferCSSPageSize===true&&c.options.printBackground===true));
    fs.writeFileSync(path.join(output,'print-smoke.json'),JSON.stringify({evidence,calls},null,2)+'\n');
    pass('PRINT all output calls ran in Electron main; PDF page size, footer, totals and hidden toolbar verified');
  } finally {
    await app.evaluate(({Menu,dialog,BrowserWindow})=>{
      const saved=globalThis.__printSmoke;
      Menu.prototype.popup=saved.popup;dialog.showSaveDialog=saved.save;
      const w=BrowserWindow.getAllWindows()[0];w.webContents.printToPDF=saved.pdf;w.webContents.print=saved.print;
      delete globalThis.__printSmoke;
    });
    await page.goto(original);
  }
};
