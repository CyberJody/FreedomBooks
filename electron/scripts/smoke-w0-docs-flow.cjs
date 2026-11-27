 'use strict';
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {spawnSync}=require('node:child_process');

module.exports=async({app,page,company,invoiceId,estimateUrl,shot,pass})=>{
  const original=page.url();
  const out=process.env.FB_SMOKE_SHOTS;
  const profile=page.locator('form[method=post]').filter({has:page.locator('#id_email')});
  const saveButton=profile.getByRole('button',{name:'Save company settings',exact:true});
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
    if(filled){for(const value of ['42 Sample Lane','Suite 7','Sampleville','Test Region','12345','Sample Country','+1 555 010 0123','billing@example.test','https://example.test','Tax ID: SYNTHETIC-TAX-123',name.includes('invoice')?'How to pay':'PAYMENT INSTRUCTIONS','Pay by check','<Sample> & test.'])assert.ok(text.stdout.includes(value),`${name}: ${value}`);}
    else assert.doesNotMatch(text.stdout,/42 Sample Lane|Tax ID:|Payment instructions|How to pay|billing@example/i);
    const info=spawnSync('pdfinfo',[file],{encoding:'utf8'});assert.equal(info.status,0,info.stderr);
    assert.match(info.stdout,/^Pages:\s+1$/m);
    fs.writeFileSync(path.join(out,name+'-pdfinfo.txt'),info.stdout);
    const render=spawnSync('pdftoppm',['-f','1','-singlefile','-scale-to','1440','-png',file,path.join(out,name+'-page1')],{encoding:'utf8'});assert.equal(render.status,0,render.stderr);
    evidence.push({name,filled,bytes:fs.statSync(file).size});
  };
  const saveSettings=async()=>{
    await saveButton.click();
    await page.getByText('Company profile saved.',{exact:true}).waitFor();
  };
  try {
    await page.goto(edit);await shot(page,'w0-settings-empty');
    await page.fill('#id_email','bad email');
    await profile.evaluate(f=>{f.noValidate=true;});
    await saveButton.click();await page.waitForSelector('.errorlist');
    assert.match(await page.locator('main').textContent(),/valid email/);
    await shot(page,'w0-settings-invalid');
    for(const [key,value] of Object.entries(details))await page.fill('#id_'+key,value);
    const empty=path.join(out,'empty-logo.png');fs.writeFileSync(empty,Buffer.alloc(0));
    await page.setInputFiles('#company-logo-file',empty);
    const tokenBeforeEmpty=await profile.locator('input[name=fb_command_token]').inputValue();
    const [emptyAlert]=await Promise.all([page.waitForEvent('dialog'),saveButton.click()]);
    assert.match(emptyAlert.message(),/file is empty/);
    assert.equal(page.url(),edit);
    assert.equal(await profile.locator('input[name=fb_command_token]').inputValue(),tokenBeforeEmpty);
    pass('W0-DOCS empty logo selection is refused before the form token is spent');
    // A misleading extension is harmless: the Rust decoder validates the actual image.
    const bad=path.join(out,'not-a-logo.png');fs.writeFileSync(bad,'<svg><script>sample</script></svg>');
    await page.setInputFiles('#company-logo-file',bad);await saveButton.click();
    await page.locator('.errorlist').filter({hasText:'Choose a PNG or JPEG logo'}).waitFor();
    assert.match(await page.locator('main').textContent(),/PNG or JPEG/);
    await page.check('#id_remove_logo');
    await saveSettings();
    await page.goto(edit);
    const logo = await page.evaluate(() => {
      const canvas = document.createElement('canvas');
      canvas.width = 120; canvas.height = 48;
      const ctx = canvas.getContext('2d');
      ctx.fillStyle = '#e1d2b8'; ctx.fillRect(0,0,120,48);
      ctx.fillStyle = '#4f2b46'; ctx.fillRect(0,0,35,48);
      return canvas.toDataURL('image/png').split(',')[1];
    });
    await page.setInputFiles('#company-logo-file',{name:'sample-logo.png',mimeType:'image/png',buffer:Buffer.from(logo,'base64')});
    await saveSettings();await page.goto(edit);
    await page.waitForFunction(()=>{const i=document.querySelector('.company-logo-preview');return i&&i.complete&&i.naturalWidth===120;});
    assert.equal(await page.inputValue('#id_street1'),details.street1);
    await shot(page,'w0-settings-filled');
    await saveButton.scrollIntoViewIfNeeded();
    await page.screenshot({path:path.join(out,'w0-settings-filled-bottom.png')});
    pass('W0-DOCS settings validate email and real image type; escaped multiline details and local logo persist');
    for(const [kind,url] of [['invoice',invoice],['estimate',estimate]]){
      await page.goto(url);await page.waitForFunction(()=>{const i=document.querySelector('.business-logo');return i&&i.complete&&i.naturalWidth===120;});
      const locality=kind==='invoice' ? ['Sampleville, Test Region 12345'] : [details.city,details.region,details.postal_code];
      assert.deepEqual(await page.locator('.business-contact').allInnerTexts(),[details.street1,details.street2,...locality,
        details.country,details.phone,details.email,details.website,'Tax ID: '+details.tax_id]);
      assert.ok(await page.locator(kind==='invoice'?'.payment-callout':'.payment-instructions').textContent().then(s=>s.includes('<Sample> & test.')));
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
      await page.goto(url);assert.equal(await page.locator('.business-logo,.business-contact,.payment-instructions,.payment-callout').count(),0);
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
