import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import os from 'node:os';
const server=new McpServer({name:'struve-diagnostics',version:'0.1.0'});
server.registerTool('runtime_info',{description:'Read-only: версия Node, ОС, CPU и доступная RAM. Не читает пользовательские файлы.',inputSchema:{},annotations:{readOnlyHint:true,destructiveHint:false}},async()=>({content:[{type:'text',text:JSON.stringify({node:process.version,platform:process.platform,cpu_threads:os.cpus().length,free_ram_gib:os.freemem()/1073741824})}]}));
await server.connect(new StdioServerTransport());
