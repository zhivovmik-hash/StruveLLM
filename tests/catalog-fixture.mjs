import {McpServer} from '@modelcontextprotocol/sdk/server/mcp.js';
import {StdioServerTransport} from '@modelcontextprotocol/sdk/server/stdio.js';
const server=new McpServer({name:'large-catalog',version:'1.0.0'});
for(let i=0;i<100;i++)server.registerTool('fixture_'+i,{description:'Large schema documentation. '.repeat(1500),inputSchema:{}},async()=>({content:[{type:'text',text:'ok'}]}));
await server.connect(new StdioServerTransport());
