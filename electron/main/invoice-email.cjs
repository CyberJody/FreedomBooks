'use strict';
// Main-only rendering of an immutable invoice snapshot. No preload, network,
// filesystem destinations or renderer-supplied PDF bytes are involved.
const {BrowserWindow,session} = require('electron');
const guards = require('./guards.cjs');
const PRINT_CSP = "default-src 'none'; script-src 'none'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'none'; object-src 'none'; frame-src 'none'; worker-src 'none'; base-uri 'none'; form-action 'none'";

function controller(native,staticRoot,exclusive) {
  return async function deliver(ticket) {
    if (!/^[0-9a-f-]{36}$/.test(ticket)) throw new Error('Invalid email preparation.');
    const generation=JSON.parse(native.appState()).session_generation;
    const current=()=>{if(generation!==JSON.parse(native.appState()).session_generation) throw new Error('The active user changed.');};
    const partition=session.fromPartition(`invoice-email-${ticket}`,{cache:false});
    let win,html,registered=false;
    try {
      current();html=native.invoiceEmailPrint(ticket);current();
      guards.session(partition);
      partition.protocol.handle('fb',request=>{
        if(generation!==JSON.parse(native.appState()).session_generation) return new Response('Session changed',{status:403});
        const url=new URL(request.url);
        if(request.method!=='GET'||url.host!=='app') return new Response('Not found',{status:404});
        if(url.pathname==='/invoice-email-print/'&&!url.search) return new Response(html,{headers:{'content-type':'text/html; charset=utf-8','content-security-policy':PRINT_CSP,'cache-control':'no-store','x-content-type-options':'nosniff'}});
        if(url.pathname.startsWith('/static/')) return guards.staticFile(staticRoot,url.pathname.slice(8));
        return new Response('Not found',{status:404});
      });
      registered=true;
      win=new BrowserWindow({show:false,width:1000,height:1200,webPreferences:{session:partition,sandbox:true,contextIsolation:true,nodeIntegration:false,webSecurity:true,spellcheck:false}});
      win.webContents.setWindowOpenHandler(()=>({action:'deny'}));
      win.webContents.on('will-navigate',event=>event.preventDefault());
      win.webContents.on('will-attach-webview',event=>event.preventDefault());
      win.webContents.setWebRTCIPHandlingPolicy('disable_non_proxied_udp');
      await win.loadURL('fb://app/invoice-email-print/');current();
      const pdf=await win.webContents.printToPDF({printBackground:true,pageSize:'Letter',preferCSSPageSize:true});
      try {current();return JSON.parse(await exclusive(()=>native.invoiceEmailFinishAsync(ticket,pdf)));}
      finally {pdf.fill(0);}
    } catch(error) {
      // Empty bytes only close a preparation; they never contact Gmail. Once the
      // native single-use ticket was consumed this call refuses, preserving an
      // uncertain submission for the user to check.
      if(generation===JSON.parse(native.appState()).session_generation) {
        try {return JSON.parse(await exclusive(()=>native.invoiceEmailFinishAsync(ticket,Buffer.alloc(0))));} catch {}
      }
      throw error;
    } finally {
      html='';if(win&&!win.isDestroyed())win.destroy();
      if(registered)partition.protocol.unhandle('fb');
      // A cleanup failure must not replace an already recorded provider outcome.
      await Promise.allSettled([partition.clearStorageData(),partition.clearCache()]);
    }
  };
}
module.exports={controller,PRINT_CSP};
