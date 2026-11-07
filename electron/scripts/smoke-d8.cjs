async function finishRecovery(page) {
  await page.getByRole('button', {name:'Remind me later'}).click();
  await page.waitForURL(/\/settings\/book-protection\/$/);
  await Promise.all([page.waitForURL(/\/home\/$/), page.locator('a[href$="/home/"]').first().click()]);
}
'use strict';
// A second, isolated real-app flow, appended to the shared smoke without changing sibling steps.
const {_electron:electron}=require('playwright');
const fs=require('node:fs');
const path=require('node:path');
const assert=require('node:assert/strict');
module.exports=async()=>{
  const root=path.resolve(__dirname,'..');
  const data=fs.mkdtempSync(path.join(process.env.TMPDIR,'fb-d8-smoke-'));
  const shots=process.env.FB_SMOKE_SHOTS || path.join(root,'staging-run');
  let checks=0;const pass=text=>console.log(`PASS D8 ${++checks}: ${text}`);
  const app=await electron.launch({ chromiumSandbox: true,args:[root],cwd:root,env:{...process.env,XDG_CONFIG_HOME:path.join(data,'config'),FB_DESKTOP_USER_DATA:data},timeout:60000});
  try{
    const page=await app.firstWindow();await page.setViewportSize({width:1440,height:1000});page.setDefaultTimeout(180000);
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.fill('#id_owner_name','Dana Whitfield');await page.fill('#id_company_name','D8 Report Supply');
    await page.selectOption('#id_starter_chart', 'empty');
    await page.check('#id_protect');await page.fill('#id_lock_password','d8-synthetic-lock');await page.fill('#id_lock_password2','d8-synthetic-lock');
    await Promise.all([page.waitForURL(/\/settings\/recovery-key\/$/),page.click('button:has-text("Create books")')]);
    await finishRecovery(page);
    pass('created an isolated encrypted book through first run');
    const company=/\/c\/([0-9a-f-]{36})\//.exec(page.url())[1];
    for (const [code,name,type] of [['100','Checking','BANK'],['400','Service income','INCOME']]) {
      await page.goto(`fb://app/c/${company}/accounts/new/`);
      await page.fill('#id_code',code);await page.fill('#id_name',name);await page.selectOption('#id_detail_type',type);
      await Promise.all([page.waitForURL(/accounts\/$/),page.click('button:has-text("Save")')]);
    }
    const ids=await app.evaluate(({app:a},c)=>{
      const rows=JSON.parse(process.getBuiltinModule('module').createRequire(a.getAppPath()+'/package.json')('./native/fb-node.node').query(JSON.stringify({type:'Accounts',company_id:c})));
      return {bank:rows.find(r=>r.code==='100').id,income:rows.find(r=>r.code==='400').id};
    },company);
    await page.goto(`fb://app/c/${company}/journals/new/`);
    await page.fill('#id_date','2026-10-10');await page.fill('#id_entry_number','REPORT-1');await page.fill('#id_memo','Report flow evidence');
    await page.selectOption('#id_lines-0-account',ids.bank);await page.fill('#id_lines-0-debit','1234.56');
    await page.selectOption('#id_lines-1-account',ids.income);await page.fill('#id_lines-1-credit','1234.56');
    await Promise.all([page.waitForURL(/journals\/[0-9a-f-]+\/$/),page.click('button:has-text("Save draft")')]);
    ids.journal=/journals\/([0-9a-f-]+)\/$/.exec(page.url())[1];
    await Promise.all([page.waitForURL(/journals\/[0-9a-f-]+\/$/),page.click('button:has-text("Post journal")')]);
    pass('seeded a posted journal through single-use forms');
    // Start each report at its real navigation link, then run its actual GET form.
    await page.click('a[href$="/reports/profit-and-loss/"]');
    await page.selectOption('[name=preset]','custom');await page.fill('[name=start]','2026-10-01');await page.fill('[name=end]','2026-10-31');
    await Promise.all([page.waitForURL(/end=2026-10-31/),page.click('button:has-text("Run report")')]);
    assert.equal(await page.locator('main h1').innerText(),'Profit and loss');assert.match(await page.locator('.fin-final').innerText(),/1,234\.56/);
    await page.screenshot({path:path.join(shots,'d8-flow-profit-loss.png')});pass('P&L runs its GET controls and renders the posted amount with grouping');
    const accountLink=page.locator(`.fin-account a[href*="account=${ids.income}"]`);
    await accountLink.click();assert.equal(await page.locator('main h1').innerText(),'General ledger');
    assert.equal(await page.inputValue('[name=account]'),ids.income);assert.match(await page.locator('.gl-report').innerText(),/REPORT-1/);
    await page.screenshot({path:path.join(shots,'d8-flow-general-ledger.png')});
    await page.locator('.gl-line .gl-num a').first().click();assert.match(page.url(),new RegExp(ids.journal));pass('an amount drills to its filtered GL, and the GL number opens the journal');
    await page.click('a[href$="/reports/balance-sheet/"]');await page.fill('[name=as_of]','2026-10-31');
    await Promise.all([page.waitForURL(/as_of=2026-10-31/),page.click('button:has-text("Run report")')]);
    assert.match(await page.locator('.report-check').innerText(),/Assets equal liabilities and equity/);
    await page.screenshot({path:path.join(shots,'d8-flow-balance-sheet.png')});pass('balance sheet runs as of the selected date and displays the equality check');
    await page.selectOption('[name=basis]','CASH');await Promise.all([page.waitForURL(/basis=CASH/),page.click('button:has-text("Run report")')]);
    assert.match(await page.locator('.report-basis').innerText(),/Cash basis/);pass('basis changes through the real GET form');
    const csv=path.join(shots,'d8-balance-sheet.csv');fs.rmSync(csv,{force:true});
    await app.evaluate(({dialog},file)=>{dialog.showSaveDialog=async()=>({canceled:false,filePath:file});},csv);
    await page.click('a:has-text("Download CSV")');
    await page.waitForFunction(()=>true); // IPC writes on the main process; poll its filesystem below.
    for(let n=0;n<100 && !fs.existsSync(csv);n++)await page.waitForTimeout(50);
    assert.ok(fs.existsSync(csv));assert.match(fs.readFileSync(csv,'utf8'),/Report,Balance sheet\r\nCompany,D8 Report Supply\r\nCurrency,USD\r\nBasis,Cash/);
    assert.equal(fs.statSync(csv).mode&0o777,0o600);pass('CSV link uses the typed export command and the native Save path');
    const csvResponse=await app.evaluate(async ({net},target)=>{
      const response=await net.fetch(target);return {status:response.status,type:response.headers.get('content-type'),disposition:response.headers.get('content-disposition'),nosniff:response.headers.get('x-content-type-options'),text:await response.text()};
    },`fb://app/c/${company}/reports/balance-sheet/?as_of=2026-10-31&output=bad&output=csv`);
    assert.equal(csvResponse.status,200);assert.equal(csvResponse.type,'text/csv; charset=utf-8');assert.match(csvResponse.disposition,/attachment/);assert.equal(csvResponse.nosniff,'nosniff');assert.match(csvResponse.text,/Report,Balance sheet/);
    pass('direct CSV URL has CSV MIME, attachment and nosniff headers, including repeated query keys');
    await page.goto(`fb://app/c/${company}/reports/general-ledger/?basis=%20CASH%20&end=bad`);
    assert.match(await page.locator('.errorlist[role=alert]').innerText(),/Select a valid choice.*Enter a valid date/s);
    assert.equal(await page.inputValue('[name=end]'),''); // date input cannot display malformed date text; the HTML retains it.
    assert.ok((await page.content()).includes('value="bad"'));
    assert.equal(await page.locator('.report-sheet').count(),0);
    await page.screenshot({path:path.join(shots,'d8-flow-invalid.png')});pass('invalid GET options retain raw HTML input and suppress the report and export actions');
    assert.deepEqual(errors,[]);pass('report navigation and packaged styling run under the production CSP without page errors');
    console.log(`SMOKE D8 OK: ${checks} checks`);
  }finally{await app.close();fs.rmSync(data,{recursive:true,force:true});}
};
if (require.main === module) module.exports().catch(error => {console.error(error.stack || error);process.exitCode=1;});
