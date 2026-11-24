import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { commandAdapter } from './command.mjs';
import {fileURLToPath} from 'node:url';
import {createRequire} from 'node:module';
let input=''; for await (const chunk of process.stdin) {input+=chunk;if(input.length>4*1024*1024)throw new Error('Input too large')}
const {config,operation,tool,arguments:args,secrets:provided={}}=JSON.parse(input);
const secrets=[];const env={...(config.environment??{})};
const headers={};
const client=new Client({name:'struve-llm',version:createRequire(import.meta.url)('../package.json').version});
const timeoutMs=Math.min(180,Math.max(1,config.timeout_seconds||30))*1000;
let transport;let timer;let finished=false;
const redact=s=>secrets.reduce((text,secret)=>text.replaceAll(secret,'[redacted]'),String(s));
async function output(value){if(finished)return;finished=true;clearTimeout(timer);try{await client.close()}catch{};const serialized=redact(JSON.stringify(value));process.stdout.write(Buffer.byteLength(serialized)>(operation==='probe'?48:3)*1024*1024?JSON.stringify({ok:false,error:operation==='probe'?'MCP catalog exceeds 48 MiB':'MCP result exceeds 3 MiB'}):serialized);}
try{
 if(operation==='call'&&(!config.enabled||!config.allowed_tools?.includes(tool)))throw new Error('Tool is not enabled');
 for(const name of config.env_names??[]){const value=provided[name]||process.env[name];if(!value)throw new Error(`Missing environment variable: ${name}`);env[name]=value;secrets.push(value)}
 if(config.bearer_env){const token=provided[config.bearer_env]||process.env[config.bearer_env];if(!token)throw new Error(`Missing environment variable: ${config.bearer_env}`);headers.Authorization=`Bearer ${token}`;secrets.push(token)}
 if(['cli','ide'].includes(config.transport)){await output({ok:true,data:await commandAdapter(config,operation,tool,args,env)});}
 else {
 const bundledNode=config.transport==='stdio'&&config.command==='@struve/node';
 const command=bundledNode?process.execPath:config.command;
 const commandArgs=(config.args??[]).map(arg=>bundledNode&&arg==='@struve/diagnostics'?fileURLToPath(new URL('./diagnostics.mjs',import.meta.url)):arg);
 transport=config.transport==='stdio'?new StdioClientTransport({command,args:commandArgs,cwd:config.cwd||undefined,env,stderr:'pipe',maxBufferSize:64*1024*1024}):new StreamableHTTPClientTransport(new URL(config.url),{requestInit:{headers}});
 if(config.transport==='stdio')transport.stderr?.resume();
 timer=setTimeout(async()=>{await output({ok:false,error:`MCP connection or request timed out (${timeoutMs/1000} seconds)`});process.exit(1)},timeoutMs);
 await client.connect(transport);
 const tools=[];let cursor;let pages=0;do{const page=await client.listTools(cursor?{cursor}:{});tools.push(...page.tools);cursor=page.nextCursor;if(++pages>20||tools.length>2000)throw new Error('MCP tool listing exceeds limit')}while(cursor);
 if(operation==='probe')await output({ok:true,data:{tools,capabilities:client.getServerCapabilities(),server:client.getServerVersion(),protocol:'MCP SDK negotiated'}});
 else if(operation==='call'){
   if(!config.enabled || !config.allowed_tools?.includes(tool))throw new Error('Tool is not enabled');
   if(!tools.some(t=>t.name===tool))throw new Error('Tool no longer exists on this server');
   const result=await client.callTool({name:tool,arguments:args??{}},undefined,{timeout:timeoutMs});if(result.isError)throw new Error(JSON.stringify(result.content));await output({ok:true,data:result});
 }else throw new Error('Unknown bridge operation');
 }
}catch(e){await output({ok:false,error:redact(e.message)})}
