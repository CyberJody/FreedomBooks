'use strict';
const {test}=require('node:test');
const assert=require('node:assert/strict');
const http=require('node:http');
const {PassThrough}=require('node:stream');
const {create,MAX}=require('../main/ai-transport.cjs');
const {serve,api,endpoint}=require('../agent/mcp.cjs');
const {refusal}=require('../agent/refusal.cjs');
const token='a'.repeat(64);
test('lost and oversized POST responses do not invite retry',async()=>{
  const huge=create({request:async()=>JSON.stringify({data:'x'.repeat(4*1024*1024)})});
  const dropped=http.createServer((req,res)=>{req.resume();req.on('end',()=>{res.writeHead(200,{'content-type':'application/json'});res.write('{');res.socket.destroy();});});
  try {
    const base=await huge.start();
    assert.equal((await raw(base)).status,413);
    await assert.rejects(api({endpoint:base,token},'/v1/call',{}),{code:'outcome_unknown'});
    await new Promise(resolve=>dropped.listen(0,'127.0.0.1',resolve));
    await assert.rejects(api({endpoint:`http://127.0.0.1:${dropped.address().port}`,token},'/v1/call',{}),{code:'outcome_unknown'});
  }finally{await huge.stop();await new Promise(resolve=>dropped.close(resolve));}
});
async function raw(base,{method='GET',route='/v1/catalogue',headers={},body}={}) {
  const url=new URL(base);
  return new Promise((resolve,reject)=>{
    const req=http.request({hostname:'127.0.0.1',port:url.port,path:route,method,agent:false,headers:{authorization:`Bearer ${token}`,...headers}},res=>{const chunks=[];res.on('data',c=>chunks.push(c));res.on('end',()=>resolve({status:res.statusCode,text:Buffer.concat(chunks).toString()}));});
    req.on('error',reject);req.end(body);
  });
}
test('loopback transport refuses browser origins, DNS rebinding, query credentials and malformed scope transport',async()=>{
  let calls=0;
  const server=create({request:async()=>{calls++;return '{}';}});
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
test('bounded body and concurrent admission',async()=>{
  let release;
  const wait=new Promise(r=>release=r);
  const server=create({request:async()=>{await wait;return '{}';}});
  const base=await server.start();
  try {
    const body='x'.repeat(MAX+1);
    assert.equal((await raw(base,{method:'POST',route:'/v1/call',headers:{'content-type':'application/json','content-length':body.length},body})).status,413);
    const first=Array.from({length:4},()=>raw(base));
    // Request arrival is observable through the independent fifth rejection; no timed sleep.
    const fifth=await raw(base);assert.equal(fifth.status,429);
    release();
    for(const r of await Promise.all(first))assert.equal(r.status,200);
  } finally {release();await server.stop();}
});
test('native/internal errors never reflect credentials or raw exceptions',async()=>{
  const server=create({request:async()=>{throw new Error(token+' /private/book');}});
  const base=await server.start();
  try {const r=await raw(base);assert.equal(r.status,400);assert.ok(!r.text.includes(token));assert.ok(!r.text.includes('/private/book'));} finally {await server.stop();}
});
test('MCP initialization, schemas, tool execution and errors use stdio JSON-RPC',async()=>{
  const requests=[];
  const server=create({request:async(_token,body)=>{
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
test('a long-running client reloads rotated credentials, preflights reads and never retries mutations',async()=>{
 let current={token:'a'.repeat(64)},saved,posts=0,reads=0;
 const server=create({generation:()=>1,request:async(token,body)=>{
  if(body===undefined){reads++;if(token===saved.token){current={...current,token:'b'.repeat(64)};throw refusal('connection_changed');}assert.equal(token,current.token);return '{"operations":[]}';}
  posts++;throw Error('Synthetic response lost after posting');
 }});
 const base=await server.start();saved={endpoint:base,token:current.token};current={...saved};
 try{
  assert.deepEqual(await api(()=>({...current}),'/v1/catalogue'),{operations:[]});assert.equal(reads,2);
  const input=new PassThrough(),output=new PassThrough();let text='';output.on('data',c=>text+=c);
  const client=serve(()=>({...current}),input,output,{apiMode:true});input.write('{"operation":"command_SaveInvoice","arguments":{}}\n');await client.done();
  assert.equal(posts,1);assert.equal(JSON.parse(text).ok,false);assert.ok(!text.includes(current.token));
 }finally{await server.stop();}
});

test('stable codes survive both stdio adapters, including invalid and oversized input',async()=>{
  const codes=['app_not_running','books_closed','paused_signed_out','approval_gone','keyring_unavailable','busy','outcome_unknown'];
  let next=0;
  const server=create({request:async()=>{throw refusal(codes[next++]);}});
  const base=await server.start();
  try{
    const run=async(apiMode,requests)=>{
      const input=new PassThrough(),output=new PassThrough();let text='';
      output.on('data',chunk=>text+=chunk);
      const client=serve({endpoint:base,token,cookie:server.cookie},input,output,{apiMode});
      for(const request of requests)input.write(request+'\n');
      await client.done();
      return text.trim().split('\n').map(JSON.parse);
    };
    for(const [i,line] of (await run(true,['invalid','x'.repeat(MAX+1),...codes.map(()=>'{"catalogue":true}')])).entries()){
      const code=['invalid_request','request_too_large',...codes][i];
      assert.equal(line.ok,false);assert.equal(line.code,code);
      assert.equal(line.error.includes('Make a new connection'),code==='approval_gone');
    }
    next=0;
    const calls=codes.map((_,i)=>JSON.stringify({jsonrpc:'2.0',id:i,method:'tools/call',params:{name:'query_Accounts',arguments:{}}}));
    for(const [i,line] of (await run(false,calls)).entries()){
      assert.equal(line.result.isError,true);assert.equal(line.result._meta.code,codes[i]);
    }
    next=0;
    const [list]=await run(false,[JSON.stringify({jsonrpc:'2.0',id:1,method:'tools/list'})]);
    assert.equal(list.error.data.code,'app_not_running');
  }finally{await server.stop();}
  await assert.rejects(api({endpoint:base,token,cookie:server.cookie},'/v1/catalogue'),{code:'app_not_running'});
});
