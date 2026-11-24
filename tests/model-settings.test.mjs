import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import {streamChat} from '../bridge/agent.mjs';
import {systemInstructions} from '../bridge/model-settings.mjs';

async function capture(protocol,settings){
 let received;const server=http.createServer(async(req,res)=>{let body='';for await(const b of req)body+=b;received=JSON.parse(body);res.writeHead(200);res.end(protocol==='ollama'?'{"message":{"content":"ok"}}\n{"done":true}\n':'data: {"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n')});
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 try{await streamChat({protocol,endpoint:`http://127.0.0.1:${server.address().port}`,engine_model:'fixture',...settings},[{role:'system',content:'test'}],[],()=>{});return received}finally{server.closeAllConnections();await new Promise(r=>server.close(r))}
}
test('Ollama receives configured sampling, context, output, allocation and zero-valued options',async()=>{
 const body=await capture('ollama',{context_tokens:8192,max_output_tokens:256,temperature:0,top_p:.85,top_k:0,min_p:.03,repeat_penalty:1.12,repeat_last_n:-1,seed:0,frequency_penalty:.2,presence_penalty:-.1,stop:['END'],gpu_layers:0,cpu_threads:2,keep_alive_seconds:0,thinking:true});
 assert.deepEqual(body.options,{num_ctx:8192,num_predict:256,temperature:0,num_gpu:0,num_thread:2,top_p:.85,top_k:0,min_p:.03,repeat_penalty:1.12,repeat_last_n:-1,frequency_penalty:.2,presence_penalty:-.1,seed:0,stop:['END']});assert.equal(body.keep_alive,'0s');assert.equal(body.think,true);
});
test('OpenAI-compatible receives supported controls without Ollama-only options',async()=>{
 const body=await capture('openai',{temperature:.1,max_output_tokens:120,top_p:.7,seed:0,frequency_penalty:.2,presence_penalty:.3,stop:['STOP'],top_k:30,min_p:.05,repeat_penalty:1.2,gpu_layers:20});
 assert.equal(body.top_p,.7);assert.equal(body.seed,0);assert.equal(body.max_tokens,120);assert.equal(body.presence_penalty,.3);assert.deepEqual(body.stop,['STOP']);for(const key of ['top_k','min_p','options','gpu_layers','repeat_penalty'])assert.equal(body[key],undefined);
});
test('Blank sampling controls retain runtime defaults and thinking can use model default',async()=>{
 const body=await capture('ollama',{top_p:null,seed:null,stop:[],thinking:null});assert.equal(body.options.top_p,undefined);assert.equal(body.options.seed,undefined);assert.equal(body.options.stop,undefined);assert.equal(body.think,null);
});
test('Only enabled skills enter the actual system message, along with custom instructions',()=>{
 const text=systemInstructions({language:'ru',model:{system_prompt:'CUSTOM_INSTRUCTION',skills:[{name:'Enabled',content:'ENABLED_RULE',enabled:true},{name:'Disabled',content:'DISABLED_RULE',enabled:false}]}});
 assert.match(text,/CUSTOM_INSTRUCTION/);assert.match(text,/ENABLED_RULE/);assert.doesNotMatch(text,/DISABLED_RULE/);assert.match(text,/do not grant tool permissions/);
});
test('Oversized custom instructions and skills are counted against the context before network access',async()=>{
 const task={model:{protocol:'ollama',endpoint:'http://127.0.0.1:1',context_tokens:512,max_output_tokens:128,system_prompt:'x'.repeat(900)}};
 await assert.rejects(streamChat(task.model,[{role:'system',content:systemInstructions(task)}],[],()=>{}),/context estimate/);
});
