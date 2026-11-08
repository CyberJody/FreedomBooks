'use strict';
// Deliberately separate from the renderer's fb:// protocol and from the network helper.
const http = require('node:http');
const MAX = 256 * 1024;
function create({request, generation}) {
  let server = null, port = null, active = 0;
  const sockets = new Set();
  const send = (res, status, value) => {
    if (res.destroyed) return;
    const body = JSON.stringify(value);
    if (Buffer.byteLength(body) > 4 * 1024 * 1024) return send(res, 413, {error:'response_too_large'});
    res.writeHead(status, {'content-type':'application/json','cache-control':'no-store','x-content-type-options':'nosniff','connection':'close'});
    res.end(body);
  };
  async function handle(req,res) {
    // Exact numeric loopback authority defeats DNS rebinding; any browser origin/fetch metadata is refused.
    const count = key => req.rawHeaders.filter((_,i)=>i%2===0).filter(k=>k.toLowerCase()===key).length;
    if (req.socket.remoteAddress !== '127.0.0.1' || req.headers.host !== `127.0.0.1:${port}` || count('host')!==1 || count('authorization')!==1 || req.headers.origin !== undefined || Object.keys(req.headers).some(k=>k.startsWith('sec-fetch-'))) return send(res,403,{error:'forbidden'});
    if (!/^Bearer [0-9a-f]{64}$/.test(req.headers.authorization || '')) return send(res,401,{error:'unauthorized'});
    const read = req.method==='GET' && req.url==='/v1/catalogue';
    const write = req.method==='POST' && req.url==='/v1/call';
    if (!read && !write) return send(res,404,{error:'not_found'});
    if (write && (req.headers['content-type']!=='application/json' || count('content-type')!==1)) return send(res,415,{error:'json_required'});
    if (read && (req.headers['transfer-encoding'] || Number(req.headers['content-length'] || 0)!==0)) return send(res,400,{error:'invalid_request'});
    if (active>=4) return send(res,429,{error:'busy'});
    active++;
    try {
      let bytes=0; const chunks=[];
      for await (const chunk of req) {
        bytes+=chunk.length;
        if (bytes>MAX) { send(res,413,{error:'request_too_large'}); return; }
        chunks.push(chunk);
      }
      const before=generation();
      const value=await request(req.headers.authorization.slice(7), read ? undefined : Buffer.concat(chunks).toString('utf8'));
      // A deferred desktop lock can run as the worker finishes. Never deliver book data afterwards.
      if (before!==generation()) return send(res,401,{error:'session_changed'});
      const parsed=JSON.parse(value);
      if(parsed?._agent_error===true){delete parsed._agent_error;send(res,400,parsed);} else send(res,200,parsed);
    } catch {
      // Never put raw native/validation errors (which can contain book text) on an unauthenticated channel.
      send(res,400,{error:'operation_refused',message:'Check connection, permission, arguments, current revision and accounting setup. No success is implied; query current state before retrying.'});
    } finally { active--; }
  }
  return {
    async start() {
      if (server) return `http://127.0.0.1:${port}`;
      server=http.createServer({maxHeaderSize:8192,headersTimeout:5000,requestTimeout:10000,connectionsCheckingInterval:1000},(req,res)=>void handle(req,res));
      server.maxConnections=8;
      server.maxRequestsPerSocket=1;
      server.setTimeout(10000,socket=>socket.destroy());
      server.on('connection',s=>{sockets.add(s);s.on('close',()=>sockets.delete(s));});
      server.on('clientError',(_error,socket)=>socket.destroy());
      await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(0,'127.0.0.1',resolve);});
      port=server.address().port;
      return `http://127.0.0.1:${port}`;
    },
    async stop() {
      const old=server; server=null; port=null;
      for (const s of sockets) s.destroy();
      if (old) await new Promise(resolve=>old.close(resolve));
    },
  };
}
module.exports={create,MAX};
