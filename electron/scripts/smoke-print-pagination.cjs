'use strict';
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {spawnSync}=require('node:child_process');
module.exports=async({app,page,company,ids,shot,pass})=>{
  const out=process.env.FB_SMOKE_SHOTS;
  const file=path.join(out,'invoice-multipage.pdf');
  await page.goto(`fb://app/c/${company}/invoices/new/`);
  await page.selectOption('#id_customer',ids.customer);
  await page.fill('#id_date','2026-10-31');await page.fill('#id_due_date','2026-11-30');
  await page.selectOption('#id_tax_mode','NONE');
  while(Number(await page.inputValue('#id_lines-TOTAL_FORMS'))<40)await page.click('#add-line');
  for(let i=0;i<40;i++){
    await page.fill(`#id_lines-${i}-description`,`Line ${String(i+1).padStart(2,'0')}: Synthetic invoice service with additional work details that wrap across several lines to verify page boundaries and repeated column headings.`);
    await page.fill(`#id_lines-${i}-quantity`,'1');await page.fill(`#id_lines-${i}-rate`,'25.00');
    await page.selectOption(`#id_lines-${i}-income_account`,ids.income);
  }
  await Promise.all([page.waitForURL(/invoices\/[0-9a-f-]{36}\/$/),page.click('button:has-text("Save invoice")')]);
  await page.getByRole('link',{name:'Print / Save as PDF',exact:true}).first().click();await page.waitForURL(/\/print\/$/);
  const number=(await page.locator('.meta-box td').last().innerText()).trim();
  assert.equal(await page.textContent('#print-total'),'1,000.00');
  await shot(page,'print-live-invoice-multipage');
  fs.rmSync(file,{force:true});
  await app.evaluate(({Menu,dialog},file)=>{
    globalThis.__longPrint={popup:Menu.prototype.popup,save:dialog.showSaveDialog};
    Menu.prototype.popup=function(options){this.items.find(i=>i.label==='Save as PDF…').click();options.callback();};
    dialog.showSaveDialog=async()=>({canceled:false,filePath:file});
  },file);
  try{
    await page.click('#print-now');await page.waitForFunction(()=>!document.querySelector('#print-now').disabled);
    assert.ok(fs.existsSync(file));
    const info=spawnSync('pdfinfo',[file],{encoding:'utf8'});assert.equal(info.status,0,info.stderr);
    const pages=Number(/Pages:\s+(\d+)/.exec(info.stdout)[1]);assert.ok(pages>=2);
    const text=spawnSync('pdftotext',['-layout',file,'-'],{encoding:'utf8'});assert.equal(text.status,0,text.stderr);
    assert.ok(text.stdout.includes(number));assert.match(text.stdout,/Total USD\s+1,000\.00/);
    assert.equal((text.stdout.match(/Line \d{2}:/g)||[]).length,40);
    for(let i=1;i<=pages;i++)assert.ok(text.stdout.includes(`Page ${i} of ${pages}`),`footer on page ${i}`);
    fs.writeFileSync(path.join(out,'invoice-multipage.txt'),text.stdout);
    fs.writeFileSync(path.join(out,'invoice-multipage-pdfinfo.txt'),info.stdout);
    for(let i=1;i<=pages;i++){
      const render=spawnSync('pdftoppm',['-f',String(i),'-singlefile','-scale-to','1440','-png',file,path.join(out,`invoice-multipage-page${i}`)],{encoding:'utf8'});
      assert.equal(render.status,0,render.stderr);
    }
    fs.writeFileSync(path.join(out,'print-pagination.json'),JSON.stringify({file,number,pages,rows:40,total:'1000.00',footersEveryPage:true},null,2)+'\n');
    pass(`PRINT multipage invoice: ${pages} Letter pages, 40 complete rows, totals and every numbered footer verified`);
  }finally{
    await app.evaluate(({Menu,dialog})=>{Menu.prototype.popup=globalThis.__longPrint.popup;dialog.showSaveDialog=globalThis.__longPrint.save;delete globalThis.__longPrint;});
  }
};
