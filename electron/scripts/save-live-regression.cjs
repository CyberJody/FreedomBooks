'use strict';
const fs=require('node:fs');const path=require('node:path');const assert=require('node:assert/strict');
const root=path.resolve(__dirname,'..');
const out=process.env.FB_SMOKE_SHOTS;
fs.mkdirSync(path.join(out,'tmp'),{recursive:true});
const hash=file=>require('node:crypto').createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const {_electron:electron}=require(root+'/node_modules/playwright');
(async()=>{
  const evidence=[];
  for(const variant of ['book','pdf-symlink']){
    const data=fs.mkdtempSync(path.join(out,'tmp/live-save-'));
    const app=await electron.launch({args:[root],cwd:root,env:{...process.env,XDG_CONFIG_HOME:path.join(data,'config'),FB_DESKTOP_USER_DATA:data},timeout:60000});
    try {
      const page=await app.firstWindow();page.setDefaultTimeout(60000);
      const alerts=[];page.on('dialog',async dialog=>{alerts.push(dialog.message());await dialog.dismiss();});
      await page.fill('#id_owner_name','Review');await page.fill('#id_company_name','Synthetic save-path review');
      await page.fill('#id_lock_password','review-only-lock');await page.fill('#id_lock_password2','review-only-lock');
      await Promise.all([page.waitForURL(/\/home\/$/),page.click('button:has-text("Create books")')]);
      const company=/\/c\/([^/]+)\//.exec(page.url())[1];
      await page.goto(`fb://app/c/${company}/reports/profit-and-loss/?preset=custom&start=2026-10-01&end=2026-10-31&output=print`);
      await page.waitForSelector('#print-now');
      const book=path.join(data,'books/default.fbbook');
      const before=hash(book);
      const selected=variant==='book'?book:path.join(data,'invoice.pdf');
      if(variant==='pdf-symlink')fs.symlinkSync(book,selected);
      await page.evaluate(()=>{document.title='../../books/default.fbbook';});
      await app.evaluate(({dialog,Menu},file)=>{
        globalThis.__reviewOptions=null;globalThis.__reviewError=null;
        dialog.showSaveDialog=async(_window,options)=>{globalThis.__reviewOptions=options;return {canceled:false,filePath:file};};
        dialog.showErrorBox=(_title,error)=>{globalThis.__reviewError=error;};
        Menu.prototype.popup=function(options){this.items.find(item=>item.label==='Save as PDF…').click();options.callback();};
      },selected);
      await page.click('#print-now');await page.waitForFunction(()=>!document.querySelector('#print-now').disabled);
      const state=await app.evaluate(()=>({options:globalThis.__reviewOptions,error:globalThis.__reviewError}));
      const overwritten=fs.readFileSync(book).subarray(0,5).toString()==='%PDF-';
      assert.equal(overwritten,false,'PDF must never overwrite the synthetic open book');
      assert.equal(hash(book),before,'open book SHA-256 must remain unchanged');
      const errorText=alerts.join('\n');
      assert.match(errorText,/Choose a different file name\. That location is used by FreedomBooks\./);

      evidence.push({variant,liveBookOverwrittenByPdf:overwritten,bookHashUnchanged:true,message:errorText,defaultPath:state.options.defaultPath});
      console.log(JSON.stringify(evidence.at(-1)));
      // Same print action must still support new output and replacing a confirmed regular PDF.
      const normal=path.join(out,`normal-${variant}.pdf`);fs.rmSync(normal,{force:true});
      for(const replace of [false,true]) {
        if(replace)fs.writeFileSync(normal,'OLD PDF');
        await app.evaluate(({dialog},file)=>{dialog.showSaveDialog=async()=>({canceled:false,filePath:file});},normal);
        await page.click('#print-now');await page.waitForFunction(()=>!document.querySelector('#print-now').disabled);
        assert.equal(fs.readFileSync(normal).subarray(0,5).toString(),'%PDF-');
        assert.equal(hash(book),before);
        assert.equal(fs.statSync(normal).mode&0o777,0o600);
        console.log(`SAVE PDF ${replace?'replacement':'new'} OK (${variant})`);
      }
      await page.goto(`fb://app/c/${company}/history/`);
      for(const file of [book,selected]) {
        await app.evaluate(({dialog},file)=>{dialog.showSaveDialog=async()=>({canceled:false,filePath:file});},file);
        const dismissed=page.waitForEvent('dialog');
        await page.click('a:has-text("Download this list")');
        assert.equal((await dismissed).message(),'Choose a different file name. That location is used by FreedomBooks.');
        assert.equal(hash(book),before,'CSV refuses without changing open book hash');
      }
      const csv=path.join(out,`history-${variant}.csv`);fs.rmSync(csv,{force:true});
      for(const replace of [false,true]) {
        if(replace)fs.writeFileSync(csv,'OLD CSV');
        await app.evaluate(({dialog},file)=>{dialog.showSaveDialog=async()=>({canceled:false,filePath:file});},csv);
        await page.click('a:has-text("Download this list")');
        for(let n=0;n<100 && (!fs.existsSync(csv) || fs.readFileSync(csv,'utf8')==='OLD CSV');n++)await page.waitForTimeout(50);
        assert.match(fs.readFileSync(csv,'utf8'),/When,User,Action,Document,Reason/);
        assert.equal(hash(book),before);
        console.log(`SAVE CSV ${replace?'replacement':'new'} OK (${variant})`);
      }
      evidence.push({variant,pdfNew:true,pdfReplace:true,csvRefused:true,csvNew:true,csvReplace:true,bookHashUnchanged:true});
    }finally{await app.close();fs.rmSync(data,{recursive:true});}
  }
  fs.writeFileSync(path.join(out,'save-live-results.json'),JSON.stringify(evidence,null,2)+'\n');
})().catch(error=>{console.error(error);process.exitCode=1;});
