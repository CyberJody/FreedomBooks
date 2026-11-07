'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
module.exports = async ({ app, page, company, ids, shot, pass }) => {
  try {
  const run = command => page.evaluate(c => window.fb.books.run(c), command);
  const go = p => page.goto(`fb://app/c/${company}${p}`);
  const capture = name => shot(page, `d7b-${name}`);
  const csv = async (name, expected) => {
    const file = path.join(process.env.FB_SMOKE_SHOTS || path.resolve(__dirname,'../staging-run'),name+'.csv');
    fs.rmSync(file,{force:true});
    await app.evaluate(({dialog},file)=>{globalThis.__d7bSaveDialog=dialog.showSaveDialog;dialog.showSaveDialog=async()=>({canceled:false,filePath:file});},file);
    try {
      await page.getByRole('link',{name:'CSV',exact:true}).click();
      for(let n=0;n<100&&!fs.existsSync(file);n++)await page.waitForTimeout(50);
      assert.ok(fs.existsSync(file),'CSV click must reach the native Save path');
      assert.match(fs.readFileSync(file,'utf8'),expected);
      assert.equal(fs.statSync(file).mode&0o777,0o600);
    } finally {
      await app.evaluate(({dialog})=>{dialog.showSaveDialog=globalThis.__d7bSaveDialog;delete globalThis.__d7bSaveDialog;});
    }
  };
  const submit = (name, pattern) => Promise.all([page.waitForURL(pattern), page.getByRole('button', {name, exact:true}).click()]);
  await go('/accounts/new/');
  await page.fill('#id_code','1077');
  await page.fill('#id_name','Statement checking');
  await page.selectOption('#id_detail_type','BANK');
  await submit('Save',/\/accounts\/$/);
  const bank = await page.locator('table.chart a', {hasText:'Statement checking'}).getAttribute('href').then(href=>/accounts\/([0-9a-f-]{36})/.exec(href)[1]);
  for (const [date,memo,lines] of [
    ['2026-10-01','Synthetic statement opening',[{account:bank,debit:'1000.00',credit:'0.00',description:'Opening',name:'Opening'},{account:ids.income,debit:'0.00',credit:'1000.00',description:'Opening',name:''}]],
    ['2026-10-15','Synthetic statement payment',[{account:ids.expense,debit:'200.00',credit:'0.00',description:'Statement payment',name:''},{account:bank,debit:'0.00',credit:'200.00',description:'Statement payment',name:'Pond Supply'}]],
  ]) {
    await go('/journals/new/');
    await page.fill('#id_date',date);
    await page.fill('#id_memo',memo);
    for (const [index,line] of lines.entries()) {
      for (const field of ['account','debit','credit','description','name']) {
        const selector=`#id_lines-${index}-${field}`;
        if(field==='account') await page.selectOption(selector,line[field]);
        else await page.fill(selector,line[field]);
      }
    }
    await submit('Save draft',/\/journals\/[0-9a-f-]{36}\/$/);
    await page.getByRole('button',{name:'Post journal',exact:true}).click();
    await page.waitForFunction(()=>document.querySelector('main h1').textContent.includes('Posted'));
  }
  await go('/registers/');
  await capture('01-registers');
  await page.locator(`a[href="/c/${company}/accounts/${bank}/register/"]`).click();
  await page.waitForSelector('.bank-register');
  console.log('D7b register opened');
  assert.equal(await page.textContent('#ending-balance'),'800.00');
  await page.locator('.mark-form button').first().click();
  console.log('D7b cleared button clicked');
  await page.waitForSelector('.mark-form button.on');
  assert.equal(await page.locator('.mark-form button.on').count(),1);
  await capture('02-register');
  await page.selectOption('select[name=state]','C');
  await submit('Apply',/state=C/);
  assert.equal(await page.locator('.bank-register tbody tr[id^=line-]').count(),1);
  assert.equal(await page.textContent('#ending-balance'),'800.00');
  pass('D7b bank register: cleared toggle and filters preserve the running balance');
  await csv('d7b-register-export',/Report,Register\r\nCompany,Kettle Pond Supply/);
  pass('D7b register CSV link uses the typed export command and native Save path');
  await go('/reconcile/');
  await capture('03-chooser');
  await page.locator(`a[href="/c/${company}/reconcile/${bank}/begin/"]`).click();
  await page.waitForSelector('.reconcile-begin');
  await page.fill('#id_statement_date','2026-10-31');
  await page.fill('#id_ending_balance','bad');
  await page.click('button[type=submit]');
  await page.waitForSelector('.errorlist');
  assert.equal(await page.inputValue('#id_ending_balance'),'bad');
  await page.fill('#id_ending_balance','799.00');
  await page.fill('#id_service_charge','2.00');
  await page.selectOption('#id_service_account',ids.expense);
  await page.fill('#id_interest','1.00');
  await page.selectOption('#id_interest_account',ids.income);
  await capture('04-begin');
  await submit('Continue',new RegExp(`/reconcile/${bank}/$`));
  await page.waitForSelector('#reconcile-form');
  assert.equal(await page.locator('input[name=tick]').count(),4);
  for (const tick of await page.locator('input[name=tick]').all()) await tick.check();
  assert.equal(await page.textContent('#difference'),'0.00');
  assert.equal(await page.isEnabled('#reconcile-now'),true);
  await capture('05-screen');
  await submit('Leave',/\/reconcile\/\?left=/);
  assert.match(await page.locator('main').textContent(),/Your ticks are saved/);
  await page.locator(`a[href="/c/${company}/reconcile/${bank}/"]`).click();
  await page.waitForSelector('#reconcile-form');
  assert.equal(await page.locator('input[name=tick]:checked').count(),4);
  assert.equal(await page.textContent('#difference'),'0.00');
  pass('D7b finish later: Leave saves ticks and Continue resumes the same zero-difference statement');
  await submit('Reconcile now',/\/reconciliations\/[0-9a-f-]{36}\/\?done=1/);
  await page.waitForSelector('#report-difference');
  assert.equal(await page.textContent('#report-difference'),'0.00');
  assert.equal(await page.textContent('#report-ending'),'799.00');
  assert.equal(await page.locator('.report-table tbody tr:not(.subtotal)').count(),4);
  await capture('06-report-detail');
  await page.getByRole('link',{name:'Summary',exact:true}).click();
  await page.waitForURL(/view=summary/);
  assert.equal(await page.locator('.report-table').count(),0);
  await capture('07-report-summary');
  await csv('d7b-reconciliation-export',/Section,Items,Amount\r\nChecks and payments,2,202.00/);
  const direct = await app.evaluate(async ({net},target)=>{
    const response=await net.fetch(target);return {status:response.status,type:response.headers.get('content-type'),nosniff:response.headers.get('x-content-type-options'),body:await response.text()};
  },page.url()+'&output=bad&output=csv');
  assert.equal(direct.status,200);assert.equal(direct.type,'text/csv; charset=utf-8');assert.equal(direct.nosniff,'nosniff');assert.match(direct.body,/Report,Reconciliation summary/);
  pass('D7b reconciliation CSV saves through the native dialog; direct URLs use CSV MIME and nosniff');
  await go(`/accounts/${bank}/register/`);
  assert.equal(await page.locator('.mark.reconciled').count(),4);
  pass('D7b begin → tick items → difference 0.00 → reconcile → detail/summary report and locked R marks');
  await go(`/reconcile/${bank}/history/`);
  await capture('08-history');
  await page.getByRole('link',{name:'Undo last reconciliation',exact:true}).click();
  await page.waitForSelector('#id_reason');
  await capture('09-undo');
  await page.fill('#id_reason','Synthetic statement correction');
  await submit('Undo last reconciliation',/\/history\/\?undone=/);
  assert.match(await page.locator('main').textContent(),/The reconciliation was undone/);
  await go(`/accounts/${bank}/register/`);
  assert.equal(await page.locator('.mark.reconciled').count(),0);
  assert.equal(await page.locator('.mark-form button.on').count(),4);
  pass('D7b undo restores cleared marks and retains the statement charges');
  } catch (error) {
    const out = process.env.FB_SMOKE_SHOTS || path.resolve(__dirname,'../staging-run');
    fs.writeFileSync(path.join(out,'d7b-error.html'),await page.content());
    await shot(page,'d7b-error');
    throw error;
  }
};
