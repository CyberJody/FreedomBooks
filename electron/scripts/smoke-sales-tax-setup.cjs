'use strict';
// Real Electron, isolated synthetic book. Requires FB_NATIVE_MODULE and a fresh absolute output path.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {_electron:electron}=require('playwright');
const root=path.resolve(__dirname,'..'),out=process.argv[2];
assert.ok(out&&path.isAbsolute(out)&&!fs.existsSync(out));fs.mkdirSync(out,{recursive:true});
assert.ok(process.env.FB_NATIVE_MODULE&&process.env.DISPLAY&&![':0',':0.0',':20',':20.0'].includes(process.env.DISPLAY));
(async()=>{
 const app=await electron.launch({chromiumSandbox:true,args:[root,'--password-store=basic'],cwd:root,env:{...process.env,FB_DESKTOP_USER_DATA:path.join(out,'profile'),XDG_CONFIG_HOME:path.join(out,'config'),XDG_DATA_HOME:path.join(out,'data'),XDG_CACHE_HOME:path.join(out,'cache'),XDG_STATE_HOME:path.join(out,'state')},timeout:60000});
 let page;const errors=[];
 try {
  page=await app.firstWindow();page.setDefaultTimeout(30000);page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:1000});
  await page.fill('#id_owner_name','Invoice tester');await page.fill('#id_company_name','Synthetic invoice company');await page.selectOption('#id_starter_chart','empty');await page.getByRole('button',{name:'Create books',exact:true}).click();await page.waitForURL(/\/desktop\/sign-in\/$/);
  await page.fill('#id_password','SyntheticInvoiceUser42');await page.fill('#id_password_repeat','SyntheticInvoiceUser42');if(await page.locator('[name="confirm_owner"]').count())await page.check('[name="confirm_owner"]');await page.getByRole('button',{name:'Set password and continue',exact:true}).click();await page.waitForURL(/\/(?:home|settings\/recovery-key)\/$/);if(page.url().endsWith('/settings/recovery-key/'))await page.locator('a[href$="/home/"]').first().click();await page.waitForURL(/\/home\/$/);
  const company=/\/c\/([^/]+)\//.exec(page.url())[1],base=`fb://app/c/${company}`;
  async function call(operation,arguments_={}){return app.evaluate(async(_,{company,operation,arguments_})=>{const n=process.mainModule.require(process.env.FB_NATIVE_MODULE),g=JSON.parse(n.aiGrant(company,'Synthetic invoice editor test','accounting_write'));try{const r=JSON.parse(await n.aiRequestAsync(g.token,JSON.stringify({operation,arguments:arguments_})));if(r.error)throw Error(JSON.stringify(r));return r;}finally{n.aiRevoke(g.id);}},{company,operation,arguments_});}
  const income=(await call('command_SaveAccount',{code:'400',name:'Services',detail_type:'INCOME'})).data.id;
  const ar=(await call('command_SaveAccount',{code:'110',name:'Receivables',detail_type:'RECEIVABLE'})).data.id;
  const liability=(await call('command_SaveAccount',{code:'210',name:'Sales tax payable',detail_type:'OTHER_CURRENT_LIABILITY'})).data.id;
  await call('command_SaveSetup',{expected_revision:0,receivables_id:ar,collected_tax_id:liability});
  const customer=(await call('command_SaveDirectoryRecord',{values:{kind:'customer',display_name:'Example Workshop',business_name:'Example Workshop',contact_name:'Casey Sample',email:'casey@example.invalid',phone:'',billing_address:'100 Example Street, Sample City',notes:'',tax_status:'TAXABLE'}})).data.id;
  assert.match(await page.locator('body').innerText(),/Do you collect sales tax/);
  await page.goto(base+'/setup/start/');await page.selectOption('[name="charge_tax"]','yes');
  await page.fill('[name="name"]','Sample tax');await page.fill('[name="rate"]','7');
  await page.fill('[name="authority_name"]','Synthetic tax agency');
  await page.screenshot({path:path.join(out,'tax-setup-desktop.png')});await page.setViewportSize({width:900,height:780});await page.screenshot({path:path.join(out,'tax-setup-small.png'),fullPage:true});
  await page.fill('[name="authority_name"]','');await page.getByRole('button',{name:'Save sales tax setup',exact:true}).click();await page.getByText('Enter a tax authority name',{exact:false}).waitFor();
  await page.fill('[name="authority_name"]','Synthetic tax agency');
  await page.getByRole('button',{name:'Save sales tax setup',exact:true}).click();await page.waitForURL(/\/setup\/start\/$/);
  assert.doesNotMatch(await page.locator('body').innerText(),/Do you collect sales tax/);
  const settings=await call('app_TaxSettings');assert.equal(settings.charge_tax,true);assert.equal(settings.owed_basis,'ACCRUAL');assert.equal(settings.filing_frequency,'MONTHLY');const tax=settings.default_item_id;assert.ok(tax);
  const values={kind:'item',code:'On-site service',description:'On-site support and maintenance',item_kind:'SERVICE',sale_rate:'129.00',income_account:income,taxable:true};
  const item=(await call('command_SaveDirectoryRecord',{values})).data.id;
  const discount=(await call('command_SaveDirectoryRecord',{values:{...values,code:'Customer discount',description:'Preferred customer discount',sale_rate:'0.00'}})).data.id;
  for(let i=0;i<15;i++)await call('command_SaveDirectoryRecord',{values:{...values,code:`Service ${String(i+1).padStart(2,'0')}`,sale_rate:'110.00'}});
  await page.goto(base+'/invoices/new/');assert.equal(await page.locator('[name="lines-0-item"] option').count(),18);
  assert.equal(await page.locator('select[name$="-income_account"]').count(),0);assert.doesNotMatch(await page.locator('body').innerText(),/snapshot options|Income account \(required/);
  await page.selectOption('[name="customer"]',customer);assert.match(await page.locator('[data-bill-to]').innerText(),/100 Example Street/);
  await page.selectOption('[name="lines-0-item"]',item);assert.equal(await page.inputValue('[name="lines-0-rate"]'),'129.00');assert.equal(await page.inputValue('[name="lines-0-quantity"]'),'1');assert.equal(await page.inputValue('[name="lines-0-income_account"]'),income);assert.equal(await page.isChecked('[name="lines-0-taxable"]'),true);
  await page.fill('[name="lines-0-quantity"]','2');await page.getByRole('button',{name:'Add discount / credit',exact:true}).click();await page.selectOption('[name="lines-1-item"]',discount);await page.fill('[name="lines-1-rate"]','25.80');
  assert.equal(await page.locator('[data-subtotal]').innerText(),'232.20');assert.equal(await page.locator('[data-tax]').innerText(),'16.25');assert.equal(await page.locator('[data-total]').innerText(),'248.45');
  await page.getByRole('button',{name:'Add note',exact:true}).click();await page.fill('[name="lines-2-description"]','Thank you for your business.\nSupport provided as agreed.');
  await page.fill('[name="memo"]','Payment due within 30 days.');
  // Removing a line then failing another field must preserve the removed row for Undo.
  await page.locator('fieldset').nth(1).getByRole('button',{name:'Remove line',exact:true}).click();await page.fill('[name="due_date"]','');await page.getByRole('button',{name:'Save draft',exact:true}).click();await page.getByText('Invoice not saved.',{exact:false}).waitFor();
  await page.locator('fieldset').nth(1).getByRole('button',{name:'Restore line',exact:true}).click();assert.equal(await page.inputValue('[name="lines-1-item"]'),discount);assert.equal(await page.isChecked('[name="lines-1-taxable"]'),true);
  await page.fill('[name="date"]','2026-11-09');await page.selectOption('[name="terms"]','30');assert.equal(await page.inputValue('[name="due_date"]'),'2026-12-09');
  await page.setViewportSize({width:900,height:780});assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  await page.locator('[name="memo"]').press('Control+s');await page.waitForURL(/\/invoices\/[^/]+\/$/);const id=/\/invoices\/([^/]+)\/$/.exec(page.url())[1];
  let saved=await call('app_Invoice',{invoice_id:id});assert.equal(saved.current.total,'248.45');assert.equal(saved.lines[0].income_account_id,income);assert.equal(saved.lines[0].item_id,item);assert.equal(saved.lines[2].line_kind,'NOTE');
  await page.goto(base+`/invoices/${id}/edit/`);await page.setViewportSize({width:1440,height:1000});await page.screenshot({path:path.join(out,'invoice-desktop.png')});
  await page.setViewportSize({width:900,height:780});assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  assert.ok(await page.locator('[name="lines-0-description"]').evaluate(e=>e.getBoundingClientRect().right<innerWidth));await page.screenshot({path:path.join(out,'invoice-small.png'),fullPage:true});
  // A changed catalog must not change an existing invoice merely by opening and saving it.
  await call('command_SaveDirectoryRecord',{record_id:item,values:{...values,description:'New catalog description',sale_rate:'999.00'}});
  await page.goto(base+`/invoices/${id}/edit/`);assert.equal(await page.inputValue('[name="lines-0-rate"]'),'129.00');await page.getByRole('button',{name:'Save draft',exact:true}).click();await page.waitForURL(/\/invoices\/[^/]+\/$/);
  let retained=await call('app_Invoice',{invoice_id:id});assert.deepEqual(retained.lines[0].item_snapshot,saved.lines[0].item_snapshot);assert.equal(retained.current.total,'248.45');
  // Deliberate reselection refreshes the snapshot, even if validation fails once.
  await page.goto(base+`/invoices/${id}/edit/`);await page.selectOption('[name="lines-0-item"]',item);await page.fill('[name="due_date"]','');await page.getByRole('button',{name:'Save draft',exact:true}).click();await page.getByText('Invoice not saved.',{exact:false}).waitFor();assert.equal(await page.inputValue('[name="lines-0-refresh_item"]'),'1');await page.fill('[name="due_date"]','2026-12-09');await page.getByRole('button',{name:'Save draft',exact:true}).click();await page.waitForURL(/\/invoices\/[^/]+\/$/);
  saved=await call('app_Invoice',{invoice_id:id});assert.equal(saved.lines[0].item_snapshot.description,'New catalog description');assert.equal(saved.current.total,'2110.25');
  await page.screenshot({path:path.join(out,'invoice-preview.png'),fullPage:true});
  const review=await call('app_ReviewInvoice',{invoice_id:id,expected_revision:saved.revision});assert.ok(review);
  const prefs=await app.evaluate(({BrowserWindow})=>{const p=BrowserWindow.getAllWindows()[0].webContents.getLastWebPreferences();return {sandbox:p.sandbox,node:p.nodeIntegration,context:p.contextIsolation};});assert.deepEqual(prefs,{sandbox:true,node:false,context:true});assert.deepEqual(errors,[]);
  fs.writeFileSync(path.join(out,'result.json'),JSON.stringify({synthetic:true,company,invoice:id,checks:['initial Home and setup sales-tax question','guided Yes and invalid retry','readonly tax settings','17 populated items','item description/rate/account/tax autofill','quantity default','taxable discount and note','remove/validation/undo','terms and keyboard save','persisted invoice arithmetic','catalog changes preserve saved prices/snapshots','reselection survives failed save','posting review','desktop/small layouts','sandbox/no page errors']},null,2));console.log('PASS invoice editor populated-item workflow');
 }catch(error){if(page&&!page.isClosed()){console.error(await page.locator('body').innerText());await page.screenshot({path:path.join(out,'failure.png'),fullPage:true});}throw error;}finally{await app.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
