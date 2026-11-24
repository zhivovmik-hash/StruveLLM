import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import os from 'node:os';
import fs from 'node:fs/promises';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {setTimeout as delay} from 'node:timers/promises';
import {fileURLToPath} from 'node:url';
import {Client} from '@modelcontextprotocol/sdk/client/index.js';
import {StdioClientTransport} from '@modelcontextprotocol/sdk/client/stdio.js';

test('MCP coordinator reads the isolated controller from its portable root', async () => {
 const source=fileURLToPath(new URL('../',import.meta.url));
 const root=await fs.mkdtemp(path.join(process.env.STRUVE_TEST_TMP||os.tmpdir(),'Struve MCP '));
 const app=path.join(root,'app');
 await fs.mkdir(app);
 const cli=path.join(app,'struve-llm.exe');
 const build=process.env.STRUVE_TEST_CLI||path.join(process.env.CARGO_TARGET_DIR||path.join(source,'target'),'release','struve-llm.exe');
 await fs.copyFile(build,cli);
 await fs.copyFile(process.env.STRUVE_TEST_NODE||process.execPath,path.join(app,'node.exe'));
 const env={...process.env,STRUVE_LLM_ROOT:root,STRUVE_LLM_SOURCE:source};
 delete env.STRUVE_LLM_PORT;
 const controller=spawn(cli,['serve'],{env,windowsHide:true,stdio:'ignore'});
 const ended=once(controller,'exit');
 const client=new Client({name:'struve-integration-check',version:'1.0.0'});
 try {
  // The MCP handshake starts a separate process and can finish before hardware
  // discovery publishes the endpoint of the controller started by this test.
  const endpoint=path.join(root,'data','controller.endpoint.json');
  const deadline=Date.now()+60_000;
  for (;;) {
   let ready=false;
   try { ready=JSON.parse(await fs.readFile(endpoint,'utf8')).pid===controller.pid; }
   catch (err) { if (err.code!=='ENOENT' && !(err instanceof SyntaxError)) throw err; }
   if (ready) break;
   if (controller.exitCode!==null || Date.now()>=deadline) {
    const log=await fs.readFile(path.join(root,'logs','controller.log'),'utf8').catch(()=>'');
    assert.fail(`Controller failed to publish its endpoint (exit ${controller.exitCode}): ${log}`);
   }
   await delay(100);
  }
  const transport=new StdioClientTransport({command:cli,args:['mcp','serve'],env,stderr:'pipe'});
  await client.connect(transport);
  assert.equal((await client.listTools()).tools.length,8);
  const answer=await client.callTool({name:'struve_status',arguments:{}});
  assert(!answer.isError,answer.content.map(item=>item.text).join('\n'));
  const status=JSON.parse(answer.content[0].text);
  assert(status.ok);
  assert.equal(status.data.controller,'ready');
  assert.equal(path.resolve(status.data.root),path.resolve(root));
 } finally {
  await client.close();
  if(controller.exitCode===null)controller.kill();
  await ended;
 }
});
