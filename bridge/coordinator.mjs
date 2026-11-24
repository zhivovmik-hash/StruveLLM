import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { z } from 'zod';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {createRequire} from 'node:module';
const run=promisify(execFile),root=process.env.STRUVE_LLM_ROOT||path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../..');
const server=new McpServer({name:'struve-controller',version:createRequire(import.meta.url)('../package.json').version});
const methods={struve_status:'doctor',struve_resources:'resources.list',struve_profiles:'profile.list',struve_projects:'project.list',struve_connections:'mcp.list',struve_plugins:'plugin.list',struve_instances:'instance.list'};
async function call(method,params={}){const {stdout}=await run(path.join(root,'app','struve-llm.exe'),['--json','request',method,'--params',JSON.stringify(params)],{windowsHide:true,timeout:15000,maxBuffer:4*1024*1024});return {content:[{type:'text',text:stdout}],isError:JSON.parse(stdout).ok!==true};}
for(const [name,method] of Object.entries(methods))server.registerTool(name,{description:`Read ${method} from the local Struve controller`,inputSchema:{},annotations:{readOnlyHint:true}},()=>call(method));
server.registerTool('struve_plan_resources',{description:'Check placement of profiles. Does not reserve or start processes.',inputSchema:{profile_ids:z.array(z.string()).min(1)},annotations:{readOnlyHint:true}},({profile_ids})=>call('resources.plan',{profile_ids}));
await server.connect(new StdioServerTransport());
