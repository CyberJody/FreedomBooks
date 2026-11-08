'use strict';
// Only the main process prints, renders PDF bytes, or asks for an output filename.
const {dialog, Menu} = require('electron');
const saves = require('./save-file.cjs');
let busy = false;

function controller(native, output = saves.create({bookPath:()=>JSON.parse(native.appState()).path}), exclusive = async work=>work()) {
  const generation = () => JSON.parse(native.appState()).session_generation;
  function requireSession(accepted) {
    if (accepted !== generation()) throw new Error('The active user changed. Open the print view again.');
  }
  function current(win, expected, accepted) {
    requireSession(accepted);
    if (!win || win.isDestroyed() || !JSON.parse(native.appState()).open) throw new Error('Unlock your books and open a print view first.');
    const url = new URL(win.webContents.getURL());
    if (url.protocol !== 'fb:' || url.host !== 'app' || url.hash) throw new Error('Open a print view first.');
    const target = url.pathname + url.search;
    if (expected && target !== expected) throw new Error('The print view changed. Open it again and retry.');
    const rendered = JSON.parse(native.render(target));
    if (rendered.status !== 200 || !rendered.html.includes('id="print-now"')) throw new Error('Open a print view first.');
    return target;
  }
  async function perform(win, action, expected, accepted = generation()) {
    if (busy) throw new Error('A print action is already open.');
    busy = true;
    try {
      requireSession(accepted);
      if (!expected && win && !win.isDestroyed() && JSON.parse(native.appState()).open) {
        const url = new URL(win.webContents.getURL());
        if (url.protocol === 'fb:' && url.host === 'app') {
          url.hash='';
          if (/^\/c\/[0-9a-f-]{36}\/(?:invoices|estimates|sales-receipts|customer-deposits)\/[0-9a-f-]{36}\/(?:revisions\/\d+\/)?$/.test(url.pathname)) {
            url.pathname += 'print/';
          } else if (/^\/c\/[0-9a-f-]{36}\/(?:reports\/(?:profit-and-loss|balance-sheet|general-ledger)|accounts\/[0-9a-f-]{36}\/register|reconciliations\/[0-9a-f-]{36})\/$/.test(url.pathname)) {
            url.searchParams.set('output','print');
          }
          const target = url.pathname + url.search;
          if (target !== new URL(win.webContents.getURL()).pathname + new URL(win.webContents.getURL()).search) {
            const rendered = JSON.parse(native.render(target));
            if (rendered.status !== 200 || !rendered.html.includes('id="print-now"')) throw new Error('Open a printable document or report first.');
            await win.loadURL(url.href);
            requireSession(accepted);
          }
        }
      }
      const target = current(win, expected, accepted);
      const recovery = target === '/settings/emergency-kit/' ? JSON.parse(native.recoveryExport()) : null;
      const options = {printBackground:true, pageSize:'Letter', landscape:/\/accounts\/[^/]+\/register\//.test(target)};
      if (action === 'print') {
        await new Promise((resolve,reject) => win.webContents.print({...options,silent:false},(ok,reason)=>ok?resolve():reject(new Error(reason || 'Printing was cancelled.'))));
        requireSession(accepted);
        if (recovery) await exclusive(() => native.recoverySavedAsync(recovery.ticket));
        requireSession(accepted);
        return {printed:true};
      }
      const title = win.webContents.getTitle().replace(/[<>:"/\\|?*\x00-\x1f]/g,'-').slice(0,120).trim() || 'FreedomBooks';
      const selected = await dialog.showSaveDialog(win,{title:'Save as PDF',defaultPath:title+'.pdf',filters:[{name:'PDF document',extensions:['pdf']}],properties:['showOverwriteConfirmation']});
      requireSession(accepted);
      if (selected.canceled || !selected.filePath) return {saved:false};
      output.validate(selected.filePath);
      current(win,target,accepted); // A lock/navigation or named-session change expires the action.
      const save = async () => {
        if (recovery) native.validateRecoveryExport(recovery.ticket);
        const pdf = await win.webContents.printToPDF({...options,preferCSSPageSize:true});
        current(win,target,accepted);
        if (recovery) native.validateRecoveryExport(recovery.ticket);
        output.write(selected.filePath,pdf);
        if (recovery) await native.recoverySavedAsync(recovery.ticket);
      };
      if (recovery) await exclusive(save); else await save();
      requireSession(accepted);
      return {saved:true};
    } finally { busy = false; }
  }
  async function choose(win, target) {
    const accepted = generation();
    current(win,target,accepted);
    const action = await new Promise(resolve=>{
      const menu = Menu.buildFromTemplate([
        {label:'Print…',click:()=>resolve('print')},
        {label:'Save as PDF…',click:()=>resolve('pdf')},
      ]);
      menu.popup({window:win,callback:()=>resolve(null)});
    });
    requireSession(accepted);
    return action ? perform(win,action,target,accepted) : {saved:false};
  }
  const menuAction = action => async (_item, window) => {
    try { await perform(window,action); }
    catch (error) { dialog.showErrorBox('Printing',String(error.message || error)); }
  };
  return {choose,perform,menuItems:[
    {label:'Print…',accelerator:'CmdOrCtrl+P',click:menuAction('print')},
    {label:'Save as PDF…',accelerator:'CmdOrCtrl+Shift+S',click:menuAction('pdf')},
  ]};
}
module.exports = {controller};
