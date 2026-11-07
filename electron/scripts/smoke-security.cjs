'use strict';
const assert=require('node:assert/strict');
// Invoke the actual registered IPC handler with the real window/frame sender.
module.exports=async({app,page,pass})=>{
  assert.equal(await page.evaluate(()=>typeof window.fb),'undefined');
  const count=await app.evaluate(async({app:a,ipcMain,BrowserWindow})=>{
      const require = process.getBuiltinModule('module').createRequire(a.getAppPath()+'/package.json');
    const path=require('node:path');
    const guards=require(path.join(a.getAppPath(),'main','guards.cjs'));
    const native=require(path.join(a.getAppPath(),'native','fb-node.node'));
    const contents=BrowserWindow.getAllWindows()[0].webContents;
    const event={sender:contents,senderFrame:contents.mainFrame};
    const handler=ipcMain._invokeHandlers.get('fb:books.run');
    let refused=0;
    for(const type of [...guards.appCommands,'UnrecognizedCommand']) {
      if(guards.rendererCommands.includes(type)) continue;
      const result=await handler(event,{type});
      if(result.ok || !/Unknown command/.test(result.error)) throw new Error(`IPC admitted ${type}`);
      refused++;
    }
    const result=await handler(event,{type:'SubmitForm',path:'/',token:'00',fields:[]});
    if(result.ok) throw new Error('IPC admitted a dead form token');
    // Native boundary also refuses a fully valid legacy command before company lookup.
    try {native.run(JSON.stringify({type:'PostInvoice',company_id:'00000000-0000-0000-0000-000000000001',invoice_id:'00000000-0000-0000-0000-000000000002',expected_revision:1}));}
    catch(e){if(/Unknown command/.test(e.message)) return refused;throw e;}
    throw new Error('native boundary admitted PostInvoice');
  });
  pass(`renderer has no page API; ${count} non-allowed IPC types, dead form token and native PostInvoice refused`);
  const company=/\/c\/([0-9a-f-]{36})\//.exec(page.url())[1];
  await page.goto(`fb://app/c/${company}/accounts/new/`);
  await page.fill('#id_code','9999');await page.fill('#id_name','Hidden\u202eName');await page.selectOption('#id_detail_type','BANK');
  await page.click('button:has-text("Save")');
  await page.waitForSelector('#id_name[aria-invalid=true]');
  assert.match(await page.locator('#id_name_error').textContent(),/Control and formatting characters are not allowed/);
  const path=require('node:path');
  await page.screenshot({path:path.join(process.env.FB_SMOKE_SHOTS,'secfix1-invisible-name.png')});
  pass('bidi account name refused with a field error under the real CSP');
};
