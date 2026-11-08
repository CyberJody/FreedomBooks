'use strict';
const {test}=require('node:test');
const assert=require('node:assert/strict');
const http=require('node:http');
const {PassThrough}=require('node:stream');
const {create,MAX}=require('../main/ai-transport.cjs');
const {serve,api,endpoint}=require('../agent/mcp.cjs');
const token='a'.repeat(64);
async function raw(base,{method='GET',route='/v1/catalogue',headers={},body}={}) {
  const url=new URL(base);
  return new Promise((resolve,reject)=>{
    const req=http.request({hostname:'127.0.0.1',port:url.port,path:route,method,agent:false,headers:{authorization:`Bearer ${token}`,...headers}},res=>{const chunks=[];res.on('data',c=>chunks.push(c));res.on('end',()=>resolve({status:res.statusCode,text:Buffer.concat(chunks).toString()}));});
    req.on('error',reject);req.end(body);
  });
}
test('loopback transport refuses browser origins, DNS rebinding, query credentials and malformed scope transport',async()=>{
  let calls=0;
  const server=create({generation:()=>1,request:async()=>{calls++;return '{}';}});
  const base=await server.start();
  try {
    assert.equal((await raw(base)).status,200);
    for(const headers of [{origin:'https://evil.example'},{origin:'null'},{host:'localhost'},{host:'evil.example'},{'sec-fetch-site':'cross-site'},{authorization:'Bearer wrong'},{authorization:''}]) {
      assert.ok((await raw(base,{headers})).status>=400);
    }
    for(const route of ['/v1/catalogue?token='+token,'/v1/call?x=1','http://evil.example/v1/catalogue']) assert.equal((await raw(base,{route})).status,404);
    assert.equal((await raw(base,{method:'OPTIONS'})).status,404);
    assert.equal((await raw(base,{method:'POST',route:'/v1/call',headers:{'content-type':'text/plain'},body:'{}'})).status,415);
    assert.equal(calls,1);
  } finally {await server.stop();}
});
test('bounded body and concurrent admission; revoked generation drops completed data',async()=>{
  let generation=1;let release;
  const wait=new Promise(r=>release=r);
  const server=create({generation:()=>generation,request:async()=>{await wait;return JSON.stringify({private:'hidden'});}});
  const base=await server.start();
  try {
    const body='x'.repeat(MAX+1);
    assert.equal((await raw(base,{method:'POST',route:'/v1/call',headers:{'content-type':'application/json','content-length':body.length},body})).status,413);
    const first=Array.from({length:4},()=>raw(base));
    // Request arrival is observable through the independent fifth rejection; no timed sleep.
    const fifth=await raw(base);assert.equal(fifth.status,429);
    generation++;release();
    for(const r of await Promise.all(first)){assert.equal(r.status,401);assert.ok(!r.text.includes('hidden'));}
  } finally {release();await server.stop();}
});
test('native/internal errors never reflect credentials or raw exceptions',async()=>{
  const server=create({generation:()=>1,request:async()=>{throw new Error(token+' /private/book');}});
  const base=await server.start();
  try {const r=await raw(base);assert.equal(r.status,400);assert.ok(!r.text.includes(token));assert.ok(!r.text.includes('/private/book'));} finally {await server.stop();}
});
test('MCP initialization, schemas, tool execution and errors use stdio JSON-RPC',async()=>{
  const requests=[];
  const server=create({generation:()=>1,request:async(_token,body)=>{
    if(body===undefined)return JSON.stringify({operations:[{name:'app_ConnectionInfo',description:'test',inputSchema:{type:'object',additionalProperties:false}}]});
    requests.push(JSON.parse(body));return JSON.stringify({company_id:'synthetic'});
  }});
  const base=await server.start();
  try {
    const input=new PassThrough(),output=new PassThrough();let text='';output.on('data',c=>text+=c);
    const mcp=serve({endpoint:base,token},input,output);
    for(const r of [{id:1,method:'initialize',params:{protocolVersion:'2025-06-18'}},{method:'notifications/initialized'},{id:2,method:'tools/list'},{id:3,method:'tools/call',params:{name:'app_ConnectionInfo',arguments:{}}},{id:4,method:'not-supported'}]) input.write(JSON.stringify({jsonrpc:'2.0',...r})+'\n');
    await mcp.done();
    const replies=text.trim().split('\n').map(JSON.parse);
    assert.equal(replies.length,4);assert.equal(replies[0].result.serverInfo.name,'freedombooks-local');assert.equal(replies[1].result.tools[0].name,'app_ConnectionInfo');assert.equal(replies[2].result.isError,false);assert.equal(replies[3].error.code,-32601);
    assert.deepEqual(requests,[{operation:'app_ConnectionInfo',arguments:{}}]);assert.ok(!text.includes(token));
    for(const bad of ['https://127.0.0.1:4','http://localhost:4','http://127.0.0.1:4/?token=secret','http://user:pass@127.0.0.1:4'])assert.throws(()=>endpoint(bad));
    assert.equal((await api({endpoint:base,token},'/v1/catalogue')).operations.length,1);
  } finally {await server.stop();}
});
test('OS-bootstrap API mode handles JSON lines; malformed MCP primitives cannot poison the next request',async()=>{
  const server=create({generation:()=>1,request:async(_token,body)=>JSON.stringify(body ? {operation:JSON.parse(body).operation}:{operations:[]})});
  const base=await server.start();
  try {
    const input=new PassThrough(),output=new PassThrough();let text='';output.on('data',c=>text+=c);
    const client=serve({endpoint:base,token},input,output,{apiMode:true});
    input.write('{"catalogue":true}\n{"operation":"query_Accounts","arguments":{}}\n');await client.done();
    const lines=text.trim().split('\n').map(JSON.parse);assert.equal(lines[0].ok,true);assert.equal(lines[1].data.operation,'query_Accounts');
    const bad=new PassThrough(),out=new PassThrough();let replies='';out.on('data',c=>replies+=c);
    const mcp=serve({endpoint:base,token},bad,out);bad.write('null\n[]\n{"jsonrpc":"2.0","id":1,"method":"ping"}\n');await mcp.done();
    assert.equal(JSON.parse(replies.trim().split('\n').at(-1)).id,1);
  } finally {await server.stop();}
});
