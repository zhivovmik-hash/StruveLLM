import {McpServer} from '@modelcontextprotocol/sdk/server/mcp.js';
import {StdioServerTransport} from '@modelcontextprotocol/sdk/server/stdio.js';
const server=new McpServer({name:'secret-redaction-test',version:'1.0.0'});
server.registerTool('echo_test_secret',{inputSchema:{}},async()=>({content:[{type:'text',text:process.env.STRUVE_SMOKE_SECRET||'missing'}]}));
await server.connect(new StdioServerTransport());
