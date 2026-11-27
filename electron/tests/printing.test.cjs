'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function fixture(route='/c/0195b4a4-55af-719e-96cb-a2f946b067bf/invoices/0195b4a4-55af-719e-96cb-a2f946b067c0/print/', overrides={},remote=null) {
  const state={open:true,generation:1,url:'fb://app'+route,calls:[],saved:[],cancel:false,afterDialog:null,afterPdf:null,afterMenu:null};
  const win={isDestroyed:()=>false,loadURL:async target=>{state.url=target;state.calls.push('navigate');},webContents:{
    getURL:()=>state.url,executeJavaScript:async()=>null,getTitle:()=> 'INV-000001 · Invoice',
    printToPDF:async options=>{state.calls.push(['pdf',options]);if(state.afterPdf)await state.afterPdf();return Buffer.from('%PDF-1.7\n');},
    print:(options,cb)=>{state.calls.push(['print',options]);cb(true);},
  }};
  const native={appState:()=>JSON.stringify({open:state.open,session_generation:state.generation}),render:target=>JSON.stringify({status:200,html:target.includes('/print/')||target.includes('output=print')||target==='/settings/emergency-kit/'?'<button id="print-now">':'<h1>Invoice'}),...overrides};
  const electron={
    dialog:{showSaveDialog:async (_win,options)=>{state.calls.push(['dialog',options]);if(state.afterDialog)await state.afterDialog();return {canceled:state.cancel,filePath:'/synthetic/invoice.pdf'};},showErrorBox:()=>{}},
    Menu:{buildFromTemplate:items=>({popup:async opts=>{if(state.afterMenu)await state.afterMenu();items[1].click();opts.callback();}})},
  };
  const output={validate:()=>{},write:(file,pdf)=>state.saved.push({file,pdf,options:{mode:0o600}})};
  const sandbox={URL,URLSearchParams,module:{exports:{}},require:name=>name==='electron'?electron:require(name.startsWith('.')?path.join(__dirname,'../main',name):name)};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../main/printing.cjs'),'utf8'),sandbox);
  return {state,win,controller:sandbox.module.exports.controller(native,output,async work=>work(),remote)};
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

test('the PDF dialog suggests a clean basename and a fixed extension', async () => {
  const {state, win, controller} = fixture();
  state.cancel = true;
  for (const [title, expected] of [
    ['../drafts/..Invoice\u202E12.PDF', 'Invoice_12.pdf'],
    ['C:\\exports\\...NUL.copy.pdf', 'FreedomBooks.pdf'],
    ['LPT².pdf', 'FreedomBooks.pdf'],
    ['... ', 'FreedomBooks.pdf'],
    ['Résumé (07).pdf', 'Résumé (07).pdf'],
    ['x'.repeat(140), 'x'.repeat(100) + '.pdf'],
  ]) {
    win.webContents.getTitle = () => title;
    await controller.perform(win, 'pdf');
    assert.equal(state.calls.at(-1)[1].defaultPath, expected);
  }
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

test('a held PDF or chooser cannot survive sign-out and same-URL sign-in',async()=>{
  for(const phase of ['afterPdf','afterDialog','afterMenu']){
    const {state,win,controller}=fixture();
    let release,entered;
    const ready=new Promise(resolve=>{entered=resolve;});
    state[phase]=()=>{entered();return new Promise(resolve=>{release=resolve;});};
    const result=controller.choose(win,new URL(state.url).pathname);
    await ready;
    // Same person or another member can return to precisely the same page in a new session.
    state.generation+=2;
    release();
    await assert.rejects(result,/session|user changed/i,phase);
    assert.equal(state.saved.length,0,phase);
    if(phase==='afterMenu')assert.equal(state.calls.length,0,'stale menu must not open a chooser');
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

test('File Print and its accelerator open the Wave 2b report print views with their filters', async () => {
  const company = '0195b4a4-55af-719e-96cb-a2f946b067bf';
  for (const [slug, filter, landscape] of [
    ['customer-statements', `customer=${company}`, false],
    ['cash-flow', 'start=2028-02-01&end=2028-02-29', false],
    ['comparative-profit-and-loss', 'period=quarterly', true],
    ['comparative-profit-and-loss', 'period=fiscal_years', false],
    ['sales-by-customer-item', 'group=customer_item', false],
    ['schedule-c', 'basis=CASH&mappings=account%7Csynthetic%7CTech+consulting%7Cgross_receipts%7C1w', false],
    ['asset-depreciation', 'start=2025-01-01&assets=Equipment%7C%7C%7C%7C2025-01-01%7C10000%7C0%7Cstraight_line%7C12%7C0', true],
  ]) {
    const page = `/c/${company}/reports/${slug}/?${filter}`;
    const printPage = `${page}&output=print`;
    for (const connected of [false, true]) {
      const marker = 'synthetic-marker';
      const remote = connected ? {
        active: () => true,
        printMarker: () => marker,
        printPage: async (target, accepted) => {
          assert.equal(target, printPage);
          assert.equal(accepted, marker);
          return {status: 200, html: '<button id="print-now">'};
        },
      } : null;
      const {state, win, controller} = fixture(
        page + (connected ? `&__fb_remote_page=${marker}` : ''), {}, remote,
      );
      const item = controller.menuItems[0];
      assert.equal(item.accelerator, 'CmdOrCtrl+P');
      await item.click(null, win);
      const expected = `fb://app${page}` + (connected ? `&__fb_remote_page=${marker}` : '') + '&output=print';
      assert.equal(state.url, expected, slug);
      assert.equal(state.calls[0], 'navigate');
      const options = state.calls.find(call => call[0] === 'print')?.[1];
      assert.equal(options?.landscape, landscape, slug);
    }
  }
});

test('File Print keeps the dates and encoded input from the displayed report print link', async () => {
  const path = '/c/0195b4a4-55af-719e-96cb-a2f946b067bf/reports/asset-depreciation/';
  const page = path + '?preset=this_month';
  const january = path + '?preset=custom&start=2028-01-01&end=2028-01-31&assets=Tool%2A~%21+shop&print=1';
  const marker = 'january-report';
  for (const connected of [false, true]) {
    const renders = [];
    const render = target => {
      renders.push(target);
      return {status: 200, html: '<button id="print-now">'};
    };
    const remote = connected ? {
      active: () => true,
      printMarker: () => marker,
      printPage: async (target, accepted) => {
        assert.equal(accepted, marker);
        return render(target);
      },
    } : null;
    const {state, win, controller} = fixture(page + (connected ? '&__fb_remote_page=' + marker : ''), {
      render: target => JSON.stringify(render(target)),
    }, remote);
    win.webContents.executeJavaScript = async script => {
      assert.match(script, /a\[data-report-print\]/);
      return connected ? january + '&__fb_remote_page=' + marker : january;
    };
    await controller.menuItems[0].click(null, win);
    assert.ok(renders.length && renders.every(target => target === january));
    assert.equal(state.url, 'fb://app' + january + (connected ? '&__fb_remote_page=' + marker : ''));
    assert.equal(state.calls.filter(call => call[0] === 'print').length, 1);

    win.webContents.executeJavaScript = async () => january + '&__fb_remote_page=wrong-marker';
    state.url = 'fb://app' + page + (connected ? '&__fb_remote_page=' + marker : '');
    await assert.rejects(controller.perform(win, 'print'), /Open a printable document or report first/);
  }
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

test('connected menu and in-page chooser use the host print view and this computer’s printer or PDF dialog', async () => {
  const marker='synthetic-marker';
  const renders=[];
  const remote={active:()=>true, printMarker:()=>marker, printPage:async (target, accepted)=>{
    assert.equal(accepted,marker);
    renders.push(target);
    return {status:200,html:'<button id="print-now">'};
  }};
  const target='/c/0195b4a4-55af-719e-96cb-a2f946b067bf/reports/balance-sheet/?output=print';
  const {win,state,controller}=fixture(target+'&__fb_remote_page='+marker,{
    render:()=>assert.fail('opened local books'), appState:()=>assert.fail('consulted local session'),
  },remote);
  await controller.menuItems[0].click(null,win);
  await controller.menuItems[1].click(null,win);
  await controller.choose(win,target);
  assert.ok(renders.length && renders.every(t=>t===target));
  assert.equal(state.saved.length,2);
  assert.equal(state.calls.filter(c=>c[0]==='print').length,1);
  win.webContents.getPrintersAsync=async()=>[];
  await assert.rejects(controller.perform(win,'print'),/No printer.*Save as PDF/);
  win.webContents.getPrintersAsync=async()=>{throw new Error('Failed to enumerate printers');};
  await assert.rejects(controller.perform(win,'print'),/Could not find a printer.*Save as PDF/);
  state.afterPdf=()=>{remote.printMarker=()=>null;};
  await assert.rejects(controller.perform(win,'pdf'),/changed/);
  assert.equal(state.saved.length,2);
});
