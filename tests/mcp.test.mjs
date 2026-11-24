import test from 'node:test';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import http from 'node:http';
import {McpServer} from '@modelcontextprotocol/sdk/server/mcp.js';
import {StreamableHTTPServerTransport} from '@modelcontextprotocol/sdk/server/streamableHttp.js';
const base=path.dirname(path.dirname(fileURLToPath(import.meta.url)));
function bridge(config,operation='probe',tool,arguments_={}){return new Promise((resolve,reject)=>{const child=spawn(process.execPath,[path.join(base,'bridge/client.mjs')],{windowsHide:true,stdio:['pipe','pipe','pipe']});let out='',err='';const timer=setTimeout(()=>{child.kill();reject(Error('test timeout'))},30000);child.stdout.on('data',x=>out+=x);child.stderr.on('data',x=>err+=x);child.on('error',reject);child.on('close',()=>{clearTimeout(timer);try{resolve(JSON.parse(out))}catch{reject(Error(err||out))}});child.stdin.end(JSON.stringify({config,operation,tool,arguments:arguments_}));})}
const config={transport:'stdio',command:process.execPath,args:[path.join(base,'bridge/diagnostics.mjs')],env_names:[],enabled:true,allowed_tools:['runtime_info']};
test('stdio discovery and real diagnostic tool call',async()=>{const p=await bridge(config);assert.equal(p.ok,true);assert.equal(p.data.tools[0].name,'runtime_info');const c=await bridge(config,'call','runtime_info');assert.equal(c.ok,true);const v=JSON.parse(c.data.content[0].text);assert.equal(v.platform,process.platform);assert(v.cpu_threads>0)});
test('disabled and unlisted tools cannot be dispatched',async()=>{for(const c of [{...config,enabled:false},{...config,allowed_tools:[]}]){const r=await bridge(c,'call','runtime_info');assert.equal(r.ok,false);assert.match(r.error,/not enabled/)}});
test('missing credential yields structured error',async()=>{const r=await bridge({...config,env_names:['STRUVE_TEST_MISSING_SECRET_0195']});assert.equal(r.ok,false);assert.match(r.error,/Missing environment/) });
test('Streamable HTTP discovery and call through a live local MCP server',async()=>{
 const listener=http.createServer(async(req,res)=>{const server=new McpServer({name:'http-fixture',version:'1.0.0'});server.registerTool('http_read',{inputSchema:{}},async()=>({content:[{type:'text',text:'HTTP transport verified'}]}));const transport=new StreamableHTTPServerTransport({sessionIdGenerator:undefined});res.on('close',()=>{transport.close();server.close()});try{await server.connect(transport);await transport.handleRequest(req,res)}catch(e){if(!res.headersSent)res.writeHead(500);res.end(String(e))}});
 await new Promise(r=>listener.listen(0,'127.0.0.1',r));
 try{const c={transport:'http',url:`http://127.0.0.1:${listener.address().port}/mcp`,env_names:[],enabled:true,allowed_tools:['http_read']};const p=await bridge(c);assert.equal(p.ok,true);assert.equal(p.data.tools[0].name,'http_read');const r=await bridge(c,'call','http_read');assert.equal(r.ok,true);assert.equal(r.data.content[0].text,'HTTP transport verified')}finally{listener.closeAllConnections();await new Promise(r=>listener.close(r))}
});

test('Configured timeout bounds an unresponsive HTTP connection',async()=>{
 const listener=http.createServer(()=>{});await new Promise(r=>listener.listen(0,'127.0.0.1',r));const started=Date.now();
 try{const r=await bridge({transport:'http',url:`http://127.0.0.1:${listener.address().port}/mcp`,timeout_seconds:1,env_names:[]});assert.equal(r.ok,false);assert.match(r.error,/timed out \(1 seconds\)/);assert(Date.now()-started<8000)}finally{listener.closeAllConnections();await new Promise(r=>listener.close(r))}
});
