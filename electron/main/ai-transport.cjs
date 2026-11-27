'use strict';
// Deliberately separate from the renderer's fb:// protocol and from the network helper.
const http = require('node:http');
const {createHmac,randomBytes} = require('node:crypto');
const {refusal,detail} = require('../agent/refusal.cjs');
const MAX = 256 * 1024;
const identity = (cookie,challenge) => createHmac('sha256',Buffer.from(cookie,'hex')).update(challenge).digest('hex');

function create({request}) {
  let server = null, port = null, starting = null, active = 0;
  const cookie = randomBytes(32).toString('hex');
  const sockets = new Set();
  const send = (res, status, value) => {
    if (res.destroyed) return;
    const body = JSON.stringify(value);
    if (Buffer.byteLength(body) > 4 * 1024 * 1024) return reject(res,413,res.req.method==='POST'?'outcome_unknown':'response_too_large');
    res.writeHead(status, {'content-type':'application/json','cache-control':'no-store','x-content-type-options':'nosniff','connection':'close'});
    res.end(body);
  };
  const reject = (res,status,code) => send(res,status,{error:code,...detail(refusal(code))});
  async function handle(req,res) {
    // Exact numeric loopback authority defeats DNS rebinding; browsers cannot reach this transport.
    const count = key => req.rawHeaders.filter((_,i)=>i%2===0).filter(k=>k.toLowerCase()===key).length;
    if (req.socket.remoteAddress !== '127.0.0.1' || req.headers.host !== `127.0.0.1:${port}` || count('host')!==1 || req.headers.origin !== undefined || Object.keys(req.headers).some(k=>k.startsWith('sec-fetch-'))) return reject(res,403,'forbidden');
    const challenge = req.headers['x-freedombooks-challenge'];
    if (req.method==='GET' && req.url==='/v1/identity' && /^[0-9a-f]{64}$/.test(challenge || '')) {
      return send(res,200,{proof:identity(cookie,challenge)});
    }
    if (count('authorization')!==1) return reject(res,403,'forbidden');
    if (!/^Bearer [0-9a-f]{64}$/.test(req.headers.authorization || '')) return reject(res,401,'approval_gone');
    const read = req.method==='GET' && req.url==='/v1/catalogue';
    const write = req.method==='POST' && req.url==='/v1/call';
    if (!read && !write) return reject(res,404,'not_found');
    if (write && (req.headers['content-type']!=='application/json' || count('content-type')!==1)) return reject(res,415,'json_required');
    if (read && (req.headers['transfer-encoding'] || Number(req.headers['content-length'] || 0)!==0)) return reject(res,400,'invalid_request');
    if (active>=4) return reject(res,429,'busy');
    active++;
    try {
      let bytes=0; const chunks=[];
      for await (const chunk of req) {
        bytes+=chunk.length;
        if (bytes>MAX) { reject(res,413,'request_too_large'); return; }
        chunks.push(chunk);
      }
      const value=await request(req.headers.authorization.slice(7), read ? undefined : Buffer.concat(chunks).toString('utf8'), req.headers['x-freedombooks-approval']);
      let parsed;
      try{parsed=JSON.parse(value);}catch{throw refusal(write?'outcome_unknown':'operation_refused');}
      if(parsed?._agent_error===true) {
        delete parsed._agent_error;
        send(res,400,parsed);
      } else send(res,200,parsed);
    } catch (error) {
      const failure=detail(error?.message==='BUSY'?refusal('busy'):error);
      send(res,failure.code==='busy'?429:400,{error:failure.code,...failure});
    } finally { active--; }
  }
  return {
    cookie,
    async start() {
      if (starting) return starting;
      if (server) return `http://127.0.0.1:${port}`;
      server=http.createServer({maxHeaderSize:8192,headersTimeout:5000,requestTimeout:10000,connectionsCheckingInterval:1000},(req,res)=>void handle(req,res));
      server.maxConnections=8;
      server.maxRequestsPerSocket=1;
      server.setTimeout(10000,socket=>socket.destroy());
      server.on('connection',s=>{sockets.add(s);s.on('close',()=>sockets.delete(s));});
      server.on('clientError',(_error,socket)=>socket.destroy());
      starting=new Promise((resolve,reject)=>{server.once('error',reject);server.listen(0,'127.0.0.1',resolve);}).then(()=>{
        port=server.address().port;
        return `http://127.0.0.1:${port}`;
      }).catch(error=>{server=null;throw error;}).finally(()=>{starting=null;});
      return starting;
    },
    async stop() {
      if(starting)try {await starting;}catch{/* Start did not publish an endpoint. */}
      const old=server; server=null; port=null;
      for (const s of sockets) s.destroy();
      if (old) await new Promise(resolve=>old.close(resolve));
    },
  };
}
module.exports={create,MAX};
