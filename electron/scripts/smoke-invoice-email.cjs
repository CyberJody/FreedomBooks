'use strict';
// Real Electron exercise of the main-only PDF capability, using synthetic HTML.
// Usage: electron scripts/smoke-invoice-email.cjs <synthetic-long-invoice.html> <new-evidence-dir>
const {app,protocol,BrowserWindow,nativeImage}=require('electron');
const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict');
const {randomUUID}=require('node:crypto');
const {spawnSync}=require('node:child_process');
const [fixture,out]=process.argv.slice(2);
assert.ok(fixture&&out);fs.mkdirSync(out,{recursive:false});
app.setPath('userData',path.join(out,'profile'));app.setPath('sessionData',path.join(out,'session'));
app.commandLine.appendSwitch('disable-background-networking');
app.commandLine.appendSwitch('disable-component-update');
protocol.registerSchemesAsPrivileged([{scheme:'fb',privileges:{standard:true,secure:true,supportFetchAPI:true}}]);
app.on('window-all-closed',()=>{});
app.whenReady().then(async()=>{
  const root=path.resolve(__dirname,'..');
  const {controller}=require('../main/invoice-email.cjs');
  const guards=require('../main/guards.cjs');
  const logo=nativeImage.createFromBitmap(Buffer.from([20,100,220,255]),{width:1,height:1}).toDataURL();
  const html=fs.readFileSync(fixture,'utf8').replace('<div class="seller">',`<div class="seller"><img id="test-logo" alt="Synthetic logo" width="40" height="40" src="${logo}">`)
    .replace('</body>','<script>globalThis.forbiddenScript=true</script><img src="https://example.invalid/leak"><iframe src="https://example.invalid/frame"></iframe></body>');
  let generation=1, calls=0, buffer, printedWindow, inspected,changeDuringPrint=false,racedPdf;
  app.on('browser-window-created',(_event,win)=>{
    printedWindow=win;
    if(changeDuringPrint){const print=win.webContents.printToPDF.bind(win.webContents);win.webContents.printToPDF=async options=>{racedPdf=await print(options);generation++;return racedPdf;};}
    const prefs=win.webContents.getLastWebPreferences();
    assert.equal(prefs.sandbox,true);assert.equal(prefs.contextIsolation,true);assert.equal(prefs.nodeIntegration,false);assert.ok(!prefs.preload);
    inspected=new Promise((resolve,reject)=>win.webContents.once('did-finish-load',()=>{
      win.webContents.executeJavaScript(`({logo:document.querySelector('#test-logo').naturalWidth,script:!!globalThis.forbiddenScript,node:typeof require,style:getComputedStyle(document.querySelector('.sheet')).display})`).then(resolve,reject);
    }));
  });
  const native={appState:()=>JSON.stringify({session_generation:generation}),invoiceEmailPrint:()=>html,
    invoiceEmailFinishAsync:async(_ticket,pdf)=>{calls++;buffer=pdf;assert.ok(pdf.subarray(0,5).equals(Buffer.from('%PDF-')));fs.writeFileSync(path.join(out,'invoice.pdf'),pdf);await assert.rejects(printedWindow.webContents.session.fetch('https://example.invalid/network-guard'),/ERR_BLOCKED_BY_CLIENT/);return JSON.stringify({type:'Redirect',location:'/synthetic/'});}};
  const result=await controller(native,path.join(root,'renderer/static'),fn=>fn())(randomUUID());
  assert.equal(result.type,'Redirect');assert.equal(calls,1);assert.ok(buffer.every(x=>x===0));assert.ok(printedWindow.isDestroyed());
  assert.deepEqual(await inspected,{logo:1,script:false,node:'undefined',style:'flex'});
  const pdf=path.join(out,'invoice.pdf');
  const text=spawnSync('pdftotext',['-layout',pdf,'-'],{encoding:'utf8'});assert.equal(text.status,0,text.stderr);
  assert.equal((text.stdout.match(/Service line \d{2}:/g)||[]).length,75);
  require('./assert-print-pagination.cjs')(pdf,['Service','line','75:']);
  const pages=Number(/Pages:\s+(\d+)/.exec(spawnSync('pdfinfo',[pdf],{encoding:'utf8'}).stdout)[1]);
  for(let i=1;i<=pages;i++)assert.ok(text.stdout.includes(`Page ${i} of ${pages}`));
  // A generation change between snapshot acquisition and rendering must not submit.
  native.invoiceEmailPrint=()=>{generation++;return html;};
  await assert.rejects(controller(native,path.join(root,'renderer/static'),fn=>fn())(randomUUID()),/active user changed/);
  assert.equal(calls,1);assert.equal(BrowserWindow.getAllWindows().length,0);
  native.invoiceEmailPrint=()=>html;changeDuringPrint=true;
  await assert.rejects(controller(native,path.join(root,'renderer/static'),fn=>fn())(randomUUID()),/active user changed/);
  assert.ok(racedPdf.length>1000);assert.ok(racedPdf.every(x=>x===0));assert.equal(calls,1);assert.equal(BrowserWindow.getAllWindows().length,0);
  fs.writeFileSync(path.join(out,'verified.json'),JSON.stringify({pages,calls,sandbox:true,logo:true,scriptBlocked:true,generationRefusal:true,printRaceCleared:true,requests:guards.requestLog()},null,2));
  console.log('PASS: isolated Electron invoice PDF, inline logo, CSP, pagination, buffer clearing and identity change');app.exit(0);
}).catch(error=>{console.error(error);app.exit(1);});
