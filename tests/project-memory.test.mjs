import test from 'node:test';
import assert from 'node:assert/strict';
import {composeChat,linkCitations} from '../bridge/agent.mjs';

test('only cited project notes become links; past history yields to the active request',()=>{
  const history=[{role:'user',content:'x'.repeat(1600)},{role:'user',content:'Which code?'}];
  const source='#/source/chat-a/task-a/user';
  const task={messages:history,model:{context_tokens:1024,max_output_tokens:256},
    project_memory:[{id:4,revision:2,text:'The code is Quartz.',source}]};
  const result=composeChat(task,'System',history,[]);
  assert.equal(result.omitted,1);
  assert.equal(result.sources[0].source,source);
  assert.ok(result.contextUsage.estimated_tokens.memory>0);
  assert.match(result.messages.at(-1).content,/Quartz.*\n.*\n\nWhich code/s);
  assert.equal(linkCitations('Quartz [M1], [M2]',result.sources,true),'Quartz [Source 1]('+source+'), [M2]');
  assert.equal(history.at(-1).content,'Which code?');
  const full=composeChat(task,'s'.repeat(1520),history,[]);
  assert.deepEqual(full.sources,[]);
});
