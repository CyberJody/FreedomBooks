async function finishRecovery(page) {
  await page.getByRole('button', {name:'Remind me later'}).click();
  await page.waitForURL(/\/settings\/book-protection\/$/);
  await Promise.all([page.waitForURL(/\/home\/$/), page.locator('a[href$="/home/"]').first().click()]);
}
'use strict';
// A real native landscape PDF check, isolated from the existing smoke's completed book.
const {_electron:electron}=require('playwright');
const fs=require('node:fs');
const path=require('node:path');
const assert=require('node:assert/strict');
const {spawnSync}=require('node:child_process');
(async()=>{
  const root=path.resolve(__dirname,'..');
  const data=fs.mkdtempSync(path.join(process.env.TMPDIR,'fb-print-register-'));
  const out=process.env.FB_SMOKE_SHOTS;
  const file=path.join(out,'register.pdf');fs.rmSync(file,{force:true});
  const app=await electron.launch({ chromiumSandbox: true,args:[root],cwd:root,env:{...process.env,XDG_CONFIG_HOME:path.join(data,'config'),FB_DESKTOP_USER_DATA:data},timeout:60000});
  try{
    const page=await app.firstWindow();await page.setViewportSize({width:1440,height:1000});page.setDefaultTimeout(180000);
    await page.fill('#id_owner_name','Dana Whitfield');await page.fill('#id_company_name','Print Register Supply');
    await page.selectOption('#id_starter_chart', 'empty');
    await page.check('#id_protect');await page.fill('#id_lock_password','print-synthetic-lock');await page.fill('#id_lock_password2','print-synthetic-lock');
    await Promise.all([page.waitForURL(/\/settings\/recovery-key\/$/),page.click('button:has-text("Create books")')]);
    await finishRecovery(page);
    const company=/\/c\/([0-9a-f-]{36})\//.exec(page.url())[1];
    for(const [code,name,type]of[['100','Checking','BANK'],['400','Service income','INCOME']]){
      await page.goto(`fb://app/c/${company}/accounts/new/`);await page.fill('#id_code',code);await page.fill('#id_name',name);await page.selectOption('#id_detail_type',type);
      await Promise.all([page.waitForURL(/accounts\/$/),page.click('button:has-text("Save")')]);
    }
    const ids=await app.evaluate(({app:a},c)=>{
      const rows=JSON.parse(process.getBuiltinModule('module').createRequire(a.getAppPath()+'/package.json')('./native/fb-node.node').query(JSON.stringify({type:'Accounts',company_id:c})));
      return {bank:rows.find(r=>r.code==='100').id,income:rows.find(r=>r.code==='400').id};
    },company);
    await page.goto(`fb://app/c/${company}/journals/new/`);
    await page.fill('#id_date','2026-10-10');await page.fill('#id_entry_number','PRINT-REGISTER-1');
    await page.fill('#id_memo','Register landscape layout with a longer memo that wraps inside its own column.');
    await page.selectOption('#id_lines-0-account',ids.bank);await page.fill('#id_lines-0-debit','1234.56');
    await page.fill('#id_lines-0-name','Synthetic customer with a longer display name');
    await page.selectOption('#id_lines-1-account',ids.income);await page.fill('#id_lines-1-credit','1234.56');
    await Promise.all([page.waitForURL(/journals\/[0-9a-f-]+\/$/),page.click('button:has-text("Save draft")')]);
    await page.click('button:has-text("Post journal")');await page.waitForFunction(()=>document.querySelector('main h1').textContent.includes('Posted'));
    await page.goto(`fb://app/c/${company}/accounts/${ids.bank}/register/`);
    await page.getByRole('link',{name:'Print',exact:true}).click();await page.waitForURL(/output=print/);
    await page.screenshot({path:path.join(out,'print-live-register.png')});
    await app.evaluate(({Menu,dialog},file)=>{
      Menu.prototype.popup=function(options){this.items.find(i=>i.label==='Save as PDF…').click();options.callback();};
      dialog.showSaveDialog=async()=>({canceled:false,filePath:file});
    },file);
    await page.click('#print-now');await page.waitForFunction(()=>!document.querySelector('#print-now').disabled);
    assert.ok(fs.existsSync(file));assert.equal(fs.statSync(file).mode&0o777,0o600);
    const info=spawnSync('pdfinfo',[file],{encoding:'utf8'});assert.equal(info.status,0,info.stderr);assert.match(info.stdout,/Page size:\s+792 x 612 pts/);
    const text=spawnSync('pdftotext',['-layout',file,'-'],{encoding:'utf8'});assert.equal(text.status,0,text.stderr);
    // Spatial layout interleaves adjacent columns when a long number/type wraps. Raw extraction
    // retains each cell's glyph order; verify both the long identity and all column content there.
    const raw=spawnSync('pdftotext',['-raw',file,'-'],{encoding:'utf8'});assert.equal(raw.status,0,raw.stderr);
    for(const pattern of [/PRINT-\s*REGISTER-1/,/1234\.56/,/General\s+journal/,/Synthetic\s+customer/,/longer\s+memo/,/Date/,/Number/,/Type/,/Payee\s+\/\s+name/,/Account/,/Memo/,/Payment/,/✓/,/Deposit/,/Balance/,/Page 1 of 1/])assert.match(raw.stdout,pattern);
    fs.writeFileSync(path.join(out,'register.txt'),text.stdout);fs.writeFileSync(path.join(out,'register-pdfinfo.txt'),info.stdout);
    const render=spawnSync('pdftoppm',['-f','1','-singlefile','-scale-to','1440','-png',file,path.join(out,'register-page1')],{encoding:'utf8'});assert.equal(render.status,0,render.stderr);
    console.log('SMOKE PRINT REGISTER OK: native Save dialog; landscape Letter 792x612; all ten columns, number, 1234.56 balance and page footer extracted');
  }finally{await app.close();fs.rmSync(data,{recursive:true,force:true});}
})().catch(error=>{console.error(error.stack||error);process.exitCode=1;});
