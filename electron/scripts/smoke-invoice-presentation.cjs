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
  const tax=(await call('command_SaveTaxRate',{name:'Sample tax',description:'Sales tax',rate:'7',new_agency_name:'Synthetic tax agency'})).data.id;
  await call('command_SaveTaxSettings',{expected_revision:0,charge_tax:true,default_item_id:tax,owed_basis:'ACCRUAL',filing_frequency:'MONTHLY'});
  const settings=await call('app_InvoiceSettings');
  const preferences={...settings.preferences.settings,expected_revision:settings.preferences.revision,zelle_enabled:true,zelle_name:'Example Workshop',zelle_recipient:'payments@example.test',zelle_instructions:'Preferred payment method. Please include the invoice number.',stripe_enabled:true,stripe_mode:'TEST',email_greeting:'Thank you for being a valued customer. Here is your latest invoice and how to pay. Your prompt payment is appreciated.',email_signature:'Thank you for your business.\nExample seller',footer:'Contact us if you need help with payment.'};
  const logo=fs.readFileSync(path.join(root,'../packaging/flatpak/icons/128x128/app.freedombooks.FreedomBooks.png')).toString('base64');
  await call('command_SaveCompanyDetails',{name:'Synthetic invoice company',currency:'USD',values:{city:'Sample City',region:'NY',email:'seller@example.test',phone:'555-555-0100',website:'example.invalid',payment_instructions:'Prefer a paper check? Email seller@example.test for more info.',logo_upload:logo,invoice_preferences:preferences}});
  const draft=(await call('command_SaveInvoice',{values:{request_key:require('node:crypto').randomUUID(),customer_id:customer,date:'2026-10-28',due_date:'2026-10-28',memo:'',tax_mode:'ITEM',tax_item_id:tax,lines:[{description:'Remote Support: testing (10/28/2026)',quantity:'1',rate:'0.50',taxable:true,income_account_id:income}]}})).data;
  const prefix=base+`/invoices/${draft.id}`;
  async function readyImages(){await page.waitForFunction(()=>[...document.images].every(i=>i.complete&&i.naturalWidth>0));}
  async function pdf(name){const bytes=await app.evaluate(async({BrowserWindow})=>Array.from(await BrowserWindow.getAllWindows()[0].webContents.printToPDF({printBackground:true,preferCSSPageSize:true})));fs.writeFileSync(path.join(out,name+'.pdf'),Buffer.from(bytes));}
  await page.goto(prefix+'/print/');await readyImages();assert.equal(await page.locator('#print-total').innerText(),'0.54');assert.match(await page.locator('body').innerText(),/Proposed amount/);assert.equal(await page.locator('a[href^="https://buy.stripe.com"]').count(),0);await page.screenshot({path:path.join(out,'draft-invoice.png'),fullPage:true});await pdf('draft-invoice');
  await page.goto(prefix+'/email-preview/');await readyImages();assert.match(await page.locator('body').innerText(),/Draft total/);assert.match(await page.locator('body').innerText(),/\$0\.54/);assert.equal(await page.locator('button[value="email_send"]').count(),0);await page.screenshot({path:path.join(out,'draft-email.png'),fullPage:true});
  await page.goto(prefix+'/edit/');
  const paymentOptions=page.locator('.invoice-payment-options');
  const paymentSummary=paymentOptions.locator('summary');
  assert.equal(await paymentSummary.innerText(),'Payment options on this invoice: Zelle® on · Stripe not connected');
  await paymentSummary.click();
  assert.match(await paymentOptions.innerText(),/payments@example\.test/);
  await page.screenshot({path:path.join(out,'editor-payment-options.png'),fullPage:true});
  // A synthetic posted invoice can show static Zelle instructions, still without a live Stripe link.
  await call('command_PostInvoice',{invoice_id:draft.id,expected_revision:draft.revision});
  await page.goto(prefix+'/print/');await readyImages();assert.equal(await page.locator('#print-balance').innerText(),'0.54');await page.screenshot({path:path.join(out,'posted-invoice.png'),fullPage:true});await pdf('posted-invoice');
  await page.goto(prefix+'/email-preview/');await readyImages();assert.match(await page.locator('body').innerText(),/Balance due/);assert.match(await page.locator('body').innerText(),/Send money with Zelle/);await page.screenshot({path:path.join(out,'posted-email.png'),fullPage:true});
  await page.setViewportSize({width:800,height:800});assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));await page.screenshot({path:path.join(out,'email-small.png'),fullPage:true});
  const long=(await call('command_SaveInvoice',{values:{request_key:require('node:crypto').randomUUID(),customer_id:customer,date:'2026-10-28',due_date:'2026-10-28',memo:'',tax_mode:'NONE',lines:[{description:Array.from({length:45},(_,i)=>`Preserved work detail ${String(i+1).padStart(2,'0')}`).join('\n'),quantity:'1',rate:'10.00',taxable:false,income_account_id:income}]}})).data;
  await page.goto(base+`/invoices/${long.id}/print/`);await readyImages();await pdf('long-invoice');
  if(process.env.FB_QR_FIXTURE){await page.setViewportSize({width:1440,height:1000});await page.setContent(fs.readFileSync(process.env.FB_QR_FIXTURE,'utf8'));await readyImages();assert.equal(await page.locator('svg[aria-label="Stripe payment link QR code"]').count(),1);await page.screenshot({path:path.join(out,'qr-invoice.png'),fullPage:true});await pdf('qr-invoice');}
  assert.deepEqual(errors,[]);
  fs.writeFileSync(path.join(out,'result.json'),JSON.stringify({synthetic:true,company,invoice:draft.id,providerCalls:false,checks:['presentation API and logo','draft total 0.54 and payment preview','no draft live links or email send','Zelle/Stripe badges and embedded assets','payment setup visible in editor','posted live balance 0.54','PDF and email screenshots','small email layout','no page errors']},null,2));console.log('PASS invoice presentation in real Electron');
 }catch(error){if(page&&!page.isClosed()){console.error(await page.locator('body').innerText());await page.screenshot({path:path.join(out,'failure.png'),fullPage:true});}throw error;}finally{await app.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
