 'use strict';
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {spawnSync}=require('node:child_process');

module.exports=async({app,page,company,invoiceId,estimateUrl,shot,pass})=>{
  const original=page.url();const out=process.env.FB_SMOKE_SHOTS;
  const edit=`fb://app/c/${company}/edit/`;
  const invoice=`fb://app/c/${company}/invoices/${invoiceId}/print/`;
  const estimate=estimateUrl+'print/';
  const details={street1:'42 Sample Lane',street2:'Suite 7',city:'Sampleville',region:'Test Region',postal_code:'12345',country:'Sample Country',phone:'+1 555 010 0123',email:'billing@example.test',website:'https://example.test',tax_id:'SYNTHETIC-TAX-123',payment_instructions:'Pay by check to Kettle Pond Supply.\nInclude the document number. <Sample> & test.'};
  const evidence=[];
  await app.evaluate(({Menu,dialog,BrowserWindow})=>{
    const w=BrowserWindow.getAllWindows()[0];
    globalThis.__w0={popup:Menu.prototype.popup,save:dialog.showSaveDialog,print:w.webContents.print,action:'pdf',printCalls:0};
    Menu.prototype.popup=function(options){this.items.find(i=>i.label===(globalThis.__w0.action==='pdf'?'Save as PDF…':'Print…')).click();options.callback();};
    dialog.showSaveDialog=async()=>({canceled:false,filePath:globalThis.__w0.file});
    w.webContents.print=function(_options,callback){globalThis.__w0.printCalls++;callback(true);};
  });
  const save=async(name,filled)=>{
    const file=path.join(out,name+'.pdf');fs.rmSync(file,{force:true});
    await app.evaluate((_e,file)=>{globalThis.__w0.file=file;globalThis.__w0.action='pdf';},file);
    await page.click('#print-now');await page.waitForFunction(()=>!document.querySelector('#print-now').disabled);
    assert.ok(fs.existsSync(file));assert.equal(fs.readFileSync(file).subarray(0,5).toString(),'%PDF-');
    const text=spawnSync('pdftotext',['-layout',file,'-'],{encoding:'utf8'});assert.equal(text.status,0,text.stderr);
    fs.writeFileSync(path.join(out,name+'.txt'),text.stdout);
    if(filled){for(const value of ['42 Sample Lane','Suite 7','Sampleville','Test Region','12345','Sample Country','+1 555 010 0123','billing@example.test','https://example.test','Tax ID: SYNTHETIC-TAX-123','PAYMENT INSTRUCTIONS','Pay by check','<Sample> & test.'])assert.ok(text.stdout.includes(value),`${name}: ${value}`);}
    else assert.doesNotMatch(text.stdout,/42 Sample Lane|Tax ID:|Payment instructions|billing@example/i);
    const info=spawnSync('pdfinfo',[file],{encoding:'utf8'});assert.equal(info.status,0,info.stderr);
    fs.writeFileSync(path.join(out,name+'-pdfinfo.txt'),info.stdout);
    const render=spawnSync('pdftoppm',['-f','1','-singlefile','-scale-to','1440','-png',file,path.join(out,name+'-page1')],{encoding:'utf8'});assert.equal(render.status,0,render.stderr);
    evidence.push({name,filled,bytes:fs.statSync(file).size});
  };
  const saveSettings=async()=>{
    await page.click('button[type=submit]');
    await page.waitForURL(`fb://app/c/${company}/`);
  };
  try {
    await page.goto(edit);await shot(page,'w0-settings-empty');
    await page.fill('#id_email','bad email');
    await page.locator('form[method=post]').evaluate(f=>{f.noValidate=true;});
    await page.click('button[type=submit]');await page.waitForSelector('.errorlist');
    assert.match(await page.locator('main').textContent(),/valid email/);
    await shot(page,'w0-settings-invalid');
    for(const [key,value] of Object.entries(details))await page.fill('#id_'+key,value);
    const empty=path.join(out,'empty-logo.png');fs.writeFileSync(empty,Buffer.alloc(0));
    await page.setInputFiles('#company-logo-file',empty);
    const tokenBeforeEmpty=await page.inputValue('input[name=fb_command_token]');
    const [emptyAlert]=await Promise.all([page.waitForEvent('dialog'),page.click('button[type=submit]')]);
    assert.match(emptyAlert.message(),/file is empty/);
    assert.equal(page.url(),edit);
    assert.equal(await page.inputValue('input[name=fb_command_token]'),tokenBeforeEmpty);
    pass('W0-DOCS empty logo selection is refused before the form token is spent');
    // A misleading extension is harmless: the Rust decoder validates the actual image.
    const bad=path.join(out,'not-a-logo.png');fs.writeFileSync(bad,'<svg><script>sample</script></svg>');
    await page.setInputFiles('#company-logo-file',bad);await page.click('button[type=submit]');
    await page.locator('.errorlist').filter({hasText:'Choose a PNG or JPEG logo'}).waitFor();
    assert.match(await page.locator('main').textContent(),/PNG or JPEG/);
    await page.check('#id_remove_logo');
    await saveSettings();
    await page.goto(edit);
    await page.setInputFiles('#company-logo-file',path.join(out,'sample-logo.png'));
    await saveSettings();await page.goto(edit);
    await page.waitForFunction(()=>{const i=document.querySelector('.company-logo-preview');return i&&i.complete&&i.naturalWidth===120;});
    assert.equal(await page.inputValue('#id_street1'),details.street1);
    await shot(page,'w0-settings-filled');
    await page.locator('button[type=submit]').scrollIntoViewIfNeeded();
    await page.screenshot({path:path.join(out,'w0-settings-filled-bottom.png')});
    pass('W0-DOCS settings validate email and real image type; escaped multiline details and local logo persist');
    for(const [kind,url] of [['invoice',invoice],['estimate',estimate]]){
      await page.goto(url);await page.waitForFunction(()=>{const i=document.querySelector('.business-logo');return i&&i.complete&&i.naturalWidth===120;});
      assert.equal(await page.locator('.business-contact').count(),10);
      assert.ok(await page.locator('.payment-instructions').textContent().then(s=>s.includes('<Sample> & test.')));
      await shot(page,'w0-'+kind+'-filled');await save('w0-'+kind+'-filled',true);
    }
    await app.evaluate(()=>{globalThis.__w0.action='print';});
    await page.click('#print-now');await page.waitForFunction(()=>!document.querySelector('#print-now').disabled);
    assert.equal(await app.evaluate(()=>globalThis.__w0.printCalls),1);
    pass('W0-DOCS filled invoice and estimate PDFs open with contact details, tax ID, logo and payment instructions; native print dispatch passes');
    await page.goto(edit);
    for(const key of Object.keys(details))await page.fill('#id_'+key,'');
    await page.check('#id_remove_logo');await saveSettings();
    for(const [kind,url] of [['invoice',invoice],['estimate',estimate]]){
      await page.goto(url);assert.equal(await page.locator('.business-logo,.business-contact,.payment-instructions').count(),0);
      await shot(page,'w0-'+kind+'-empty');await save('w0-'+kind+'-empty',false);
    }
    await page.goto(edit);assert.equal(await page.locator('.company-logo-preview').count(),0);
    fs.writeFileSync(path.join(out,'w0-docs-evidence.json'),JSON.stringify(evidence,null,2)+'\n');
    pass('W0-DOCS cleared invoice and estimate PDFs contain no extra labels, blocks or logo; removal persists');
  }finally{
    await app.evaluate(({Menu,dialog,BrowserWindow})=>{const s=globalThis.__w0;Menu.prototype.popup=s.popup;dialog.showSaveDialog=s.save;BrowserWindow.getAllWindows()[0].webContents.print=s.print;delete globalThis.__w0;});
    await page.goto(original);
  }
};
