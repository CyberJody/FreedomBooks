'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function fixture(route='/c/0195b4a4-55af-719e-96cb-a2f946b067bf/invoices/0195b4a4-55af-719e-96cb-a2f946b067c0/print/', overrides={}) {
  const state={open:true,url:'fb://app'+route,calls:[],saved:[],cancel:false,afterDialog:null,afterPdf:null};
  const win={isDestroyed:()=>false,loadURL:async target=>{state.url=target;state.calls.push('navigate');},webContents:{
    getURL:()=>state.url,getTitle:()=> 'INV-000001 · Invoice',
    printToPDF:async options=>{state.calls.push(['pdf',options]);if(state.afterPdf)state.afterPdf();return Buffer.from('%PDF-1.7\n');},
    print:(options,cb)=>{state.calls.push(['print',options]);cb(true);},
  }};
  const native={appState:()=>JSON.stringify({open:state.open}),render:target=>JSON.stringify({status:200,html:target.includes('/print/')||target.includes('output=print')||target==='/settings/emergency-kit/'?'<button id="print-now">':'<h1>Invoice'}),...overrides};
  const electron={
    dialog:{showSaveDialog:async (_win,options)=>{state.calls.push(['dialog',options]);if(state.afterDialog)state.afterDialog();return {canceled:state.cancel,filePath:'/synthetic/invoice.pdf'};},showErrorBox:()=>{}},
    Menu:{buildFromTemplate:items=>({popup:opts=>{items[1].click();opts.callback();}})},
  };
  const output={validate:()=>{},write:(file,pdf)=>state.saved.push({file,pdf,options:{mode:0o600}})};
  const sandbox={URL,module:{exports:{}},require:name=>name==='electron'?electron:require(name.startsWith('.')?path.join(__dirname,'../main',name):name)};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../main/printing.cjs'),'utf8'),sandbox);
  return {state,win,controller:sandbox.module.exports.controller(native,output,async work=>work())};
}

test('recovery PDF and printing await their asynchronous, session-bound acknowledgment',async()=>{
  for(const action of ['pdf','print']){
    let finish,started=false,settled=false;
    const ack=new Promise(resolve=>{finish=resolve;});
    const {state,win,controller}=fixture('/settings/emergency-kit/',{
      recoveryExport:()=>JSON.stringify({ticket:'synthetic-session-ticket',kit:'synthetic kit'}),
      validateRecoveryExport:ticket=>assert.equal(ticket,'synthetic-session-ticket'),
      recoverySavedAsync:async ticket=>{assert.equal(ticket,'synthetic-session-ticket');started=true;await ack;},
    });
    const result=controller.perform(win,action).finally(()=>{settled=true;});
    await new Promise(resolve=>setImmediate(resolve));
    assert.equal(started,true,'the durable acknowledgment must run on the native worker');
    assert.equal(settled,false,'do not report success before durable acknowledgment');
    finish();await result;
    assert.equal(state.saved.length,action==='pdf'?1:0);
  }
});

test('a recovery dialog cannot export or acknowledge a changed session',async()=>{
  let valid=true,ack=0;
  const {state,win,controller}=fixture('/settings/emergency-kit/',{
    recoveryExport:()=>JSON.stringify({ticket:'old-session',kit:'synthetic kit'}),
    validateRecoveryExport:()=>{if(!valid)throw new Error('Recovery export expired.');},
    recoverySavedAsync:async()=>{ack++;},
  });
  state.afterDialog=()=>{valid=false;};
  await assert.rejects(controller.perform(win,'pdf'),/expired/);
  assert.equal(state.saved.length,0);assert.equal(ack,0);
});

test('PDF Save selection produces bytes only in the main process with Letter and CSS page sizing',async()=>{
  const {state,win,controller}=fixture();
  const result=await controller.choose(win,new URL(state.url).pathname);
  assert.equal(result.saved,true);assert.equal(state.saved.length,1);
  assert.equal(state.saved[0].file,'/synthetic/invoice.pdf');assert.equal(state.saved[0].options.mode,0o600);
  const pdf=state.calls.find(c=>c[0]==='pdf')[1];assert.equal(pdf.preferCSSPageSize,true);assert.equal(pdf.pageSize,'Letter');
  assert.equal(pdf.printBackground,true);
});

test('cancelled save and lock/navigation races cannot write PDF output',async()=>{
  for(const variant of ['cancel','lock-dialog','navigate-dialog','navigate-pdf']){
    const {state,win,controller}=fixture();
    if(variant==='cancel')state.cancel=true;
    if(variant==='lock-dialog')state.afterDialog=()=>{state.open=false;};
    if(variant==='navigate-dialog')state.afterDialog=()=>{state.url='fb://app/';};
    if(variant==='navigate-pdf')state.afterPdf=()=>{state.url='fb://app/';};
    if(variant==='cancel')assert.equal((await controller.perform(win,'pdf')).saved,false);
    else await assert.rejects(controller.perform(win,'pdf'),/Unlock|changed/);
    assert.equal(state.saved.length,0,variant);
  }
});

test('File action opens a document print view and native printing honors register landscape',async()=>{
  const invoice=fixture('/c/0195b4a4-55af-719e-96cb-a2f946b067bf/invoices/0195b4a4-55af-719e-96cb-a2f946b067c0/');
  await invoice.controller.perform(invoice.win,'pdf');assert.match(invoice.state.url,/invoices\/[^/]+\/print\/$/);
  assert.equal(invoice.state.calls[0],'navigate');assert.equal(invoice.state.saved.length,1);
  const bank=fixture('/c/0195b4a4-55af-719e-96cb-a2f946b067bf/accounts/0195b4a4-55af-719e-96cb-a2f946b067c0/register/?output=print');
  await bank.controller.perform(bank.win,'print');
  const opts=bank.state.calls.find(c=>c[0]==='print')[1];assert.equal(opts.landscape,true);assert.equal(opts.silent,false);
});

test('locked, non-print and foreign pages cannot open an output dialog',async()=>{
  for(const variant of ['locked','normal','external','credit-memo']){
    const {state,win,controller}=fixture();
    if(variant==='locked')state.open=false;
    if(variant==='normal')state.url='fb://app/';
    if(variant==='credit-memo')state.url='fb://app/c/0195b4a4-55af-719e-96cb-a2f946b067bf/credit-memos/0195b4a4-55af-719e-96cb-a2f946b067c0/';
    if(variant==='external')state.url='https://example.com/print/';
    await assert.rejects(controller.perform(win,'pdf'),/Unlock|print view/);
    assert.equal(state.calls.length,0);
  }
});
