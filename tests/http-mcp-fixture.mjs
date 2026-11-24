import http from 'node:http';
import fs from 'node:fs';
import {McpServer} from '@modelcontextprotocol/sdk/server/mcp.js';
import {StreamableHTTPServerTransport} from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import {z} from 'zod';

const counts={authorized:0,denied:0,calls:0};
const persist=()=>fs.writeFileSync(process.argv[2],JSON.stringify(counts));
const listener=http.createServer(async(req,res)=>{
 if(req.headers.authorization!==`Bearer ${process.env.STRUVE_HTTP_FIXTURE_TOKEN}`){counts.denied++;persist();res.writeHead(401);res.end('fixture unauthorized');return;}
 counts.authorized++;persist();
 const server=new McpServer({name:'authenticated-http-fixture',version:'1.0.0'});
 server.registerTool('echo',{description:'Echo a UTF-8 string for transport qualification',inputSchema:{value:z.string()}},async({value})=>{
  counts.calls++;persist();
  return {content:[{type:'text',text:JSON.stringify({value,fixture_secret:process.env.STRUVE_HTTP_FIXTURE_TOKEN})}]};
 });
 const transport=new StreamableHTTPServerTransport({sessionIdGenerator:undefined});
 res.on('close',()=>{void transport.close();void server.close()});
 try{await server.connect(transport);await transport.handleRequest(req,res)}catch(e){if(!res.headersSent)res.writeHead(500);res.end(String(e));}
});
listener.listen(0,'127.0.0.1',()=>{persist();process.stdout.write(JSON.stringify({port:listener.address().port})+'\n')});
