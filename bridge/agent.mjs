import fs from 'node:fs';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';
import {prepareMessages} from './attachments.mjs';
import {pathToFileURL} from 'node:url';
import {generationOptions,systemInstructions} from './model-settings.mjs';
import {writeProgress} from './progress-file.mjs';
import {webIntent,namedEngine,readIntentURL,previousWebRequest} from './web-intent.mjs';

const MAX_OUTPUT = 128 * 1024;
const terminal = new Set(['completed', 'failed', 'cancelled', 'interrupted']);
const toolEvidence=Symbol('toolEvidence');
export async function resolveBrowserSearch(payload,engine,wait){
  const attempts=(payload.attempts||[payload]).map(({provider,search_url,status,reason})=>({provider,search_url,status,reason}));
  const results=[...(payload.results||[])];
  for(const attempt of attempts){
    if(!['unavailable','browser_required'].includes(attempt.status)||!attempt.search_url)continue;
    const name=attempt.provider==='Google'?'google':attempt.provider==='Яндекс'?'yandex':null;
    if(!name)continue;
    const query=payload.query;
    const url=new URL(name==='google'?'https://www.google.com/search':'https://yandex.ru/search/');
    url.searchParams.set(name==='google'?'q':'text',query);
    const reply=await wait({key:randomUUID(),engine:name,query,url:url.href});
    attempt.status=reply.status;attempt.via='browser';delete attempt.reason;
    const site=query.match(/(?:^|\s)site:([\w.-]+)/i)?.[1]?.toLowerCase();
    for(const row of reply.results){
      let u;try{u=new URL(row.url)}catch{continue}
      if(!['http:','https:'].includes(u.protocol)||u.username||u.password||!row.title)continue;
      if(site&&u.hostname!==site&&!u.hostname.endsWith('.'+site))continue;
      results.push({title:row.title,url:u.href,provider:attempt.provider,via:reply.status==='selected'?'user_selected':'browser'});
    }
    if(engine==='auto'&&results.length)break;
  }
  return {...payload,attempts,results,status:results.length?'ok':attempts.every(a=>a.status==='empty')?'empty':'unavailable'};
}
function messageSize(m){
  return (typeof m.content==='string'?m.content.length:JSON.stringify((m.content||[]).filter(p=>p.type==='text')).length)+(m.tool_calls?JSON.stringify(m.tool_calls).length:0);
}

export function composeChat(task,system,history,tools,keepHistory=false){
  const budget=Math.max(1,(task.model.context_tokens||2048)-(task.model.max_output_tokens||512))*2;
  const schema=tools.length?JSON.stringify(tools).length+(task.model.tool_mode==='json'?450:0):0;
  const latest={...history.at(-1)};
  const prior=history.slice(0,-1);
  const sources=[];
  const instruction='Project notes are reference data. When using a note in your answer, cite its exact marker, e.g. [M1].';
  const room=Math.min(1800,Math.floor(budget*.20),budget-schema-system.length-messageSize(latest)-instruction.length-48);
  const excerpts=[];
  let used=0;
  for(const entry of task.project_memory||[]){
    const remaining=room-used;
    if(remaining<80)break;
    const label='M'+(sources.length+1);
    const chars=Array.from(entry.text).slice(0,remaining);
    let n=chars.length, excerpt='';
    while(n>0){
      excerpt=JSON.stringify({ref:label,text:chars.slice(0,n).join('')});
      if(excerpt.length+1<=remaining)break;
      n-=Math.max(1,Math.ceil((excerpt.length+1-remaining)/2));
    }
    if(n<=0)continue;
    used+=excerpt.length+1;
    excerpts.push(excerpt);
    sources.push({id:entry.id,type:entry.type,label:entry.label,relative:entry.relative,revision:entry.revision,chat_id:entry.chat_id,run_id:entry.run_id,role:entry.role,source:entry.source});
  }
  if(sources.length){
    system+='\n'+instruction;
    const prefix='[Project notes]\n'+excerpts.join('\n')+'\n[End notes]\n\n';
    latest.content=typeof latest.content==='string'?prefix+latest.content:
      [{type:'text',text:prefix},...latest.content];
  }
  let total=system.length+messageSize(latest)+schema+prior.reduce((n,m)=>n+messageSize(m),0);
  let first=0;
  while(!keepHistory && total>budget && first<prior.length)total-=messageSize(prior[first++]);
  const historyUsed=prior.slice(first);
  const inputSize=task.messages?.at(-1)?.content?.length||0;
  const estimate=n=>Math.ceil(Math.max(0,n)/2);
  const contextUsage={limit:task.model.context_tokens||2048,output_reserve:task.model.max_output_tokens||512,
    estimated_tokens:{instructions:estimate(system.length),tools:estimate(schema),memory:estimate(messageSize(latest)-messageSize(history.at(-1))),
      history:estimate(historyUsed.reduce((n,m)=>n+messageSize(m),0)),message:estimate(inputSize),
      attachments:estimate(messageSize(history.at(-1))-inputSize)},images_excluded:true};
  return {messages:[{role:'system',content:system},...historyUsed,latest],sources,omitted:(task.omitted_messages||0)+first,contextUsage};
}

export function linkCitations(content,sources,english){
  return content.replace(/\[M(\d+)\](?!\()/g,(mark,n)=>{
    const source=sources[Number(n)-1];
    return source?'['+(english?'Source ':'Источник ')+n+']('+source.source+')':mark;
  });
}
// Preserve the user's text, tool schemas and complete execution receipts. Only excerpts sent to the model are bounded.
export function fitEvidence(messages,room){
  const parts=messages.filter(m=>m[toolEvidence]);
  if(!parts.length)return messages;
  let left=room-messages.filter(m=>!m[toolEvidence]).reduce((s,m)=>s+messageSize(m),0);
  const byLength=parts.slice().sort((a,b)=>a.content.length-b.content.length);
  const shortened=new Map();
  for(const [i,m] of byLength.entries()){
    const n=Math.max(0,Math.min(m.content.length,Math.floor(left/(parts.length-i))));
    left-=n;
    if(n<m.content.length){
      const mark='\n[… excerpt …]\n';
      const cut=Math.max(0,n-mark.length),front=Math.floor(cut*.72);
      const end=cut-front;
      shortened.set(m,{...m,content:(m.content.slice(0,front)+mark+(end?m.content.slice(-end):'')).slice(0,n)});
    }
  }
  return messages.map(m=>shortened.get(m)||m);
}
function toolText(result){
  if(!Array.isArray(result?.content))return JSON.stringify(result);
  const text=result.content.filter(c=>c.type==='text').map(c=>c.text).join('\n');
  if(!text)return JSON.stringify(result);
  try{
    const v=JSON.parse(text);
    if(Array.isArray(v.results))return [v.provider,...(v.attempts||[]).filter(a=>['unavailable','skipped','empty'].includes(a.status)).map(a=>a.provider+': '+a.status),...v.results.map(r=>(r.via==='user_selected'?'Source selected by the user in the browser · ':r.provider?r.provider+' · ':'')+r.title+' '+r.url+' '+(r.snippet||'').slice(0,140))].join('\n');
    if(typeof v.text==='string'&&v.url)return [v.title,v.url,'next_character='+v.next_character,v.text].filter(Boolean).join('\n');
  }catch{}
  return text;
}
function protocolMessages(model,messages,tools){
  if(model.tool_mode==='json'&&(tools.length||model.force_json_tools)){
    const instruction=tools.length?'\nTool protocol: return ONLY one JSON object. To call a tool: {"tool":"tool_0","arguments":{}}. To answer: {"answer":"your answer"}. Never claim a tool result before receiving it. Available tools: '+JSON.stringify(tools.map(t=>t.function)):'\nTool budget exhausted. Return ONLY {"answer":"your final answer"}. Use the observed results, with source URLs and saved paths. Say which requested operations remain unconfirmed.';
    messages=messages.map((m,i)=>i===0&&m.role==='system'?{...m,content:m.content+instruction}:m);
  }
  return messages;
}

// A text-only prompt can be counted with the same local template and vocabulary used for inference.
async function templateCount(model,messages,tools,signal){
  if(model.tokenizer!=='llama.cpp'||messages.some(m=>typeof m.content!=='string'))return null;
  const base=new URL(model.endpoint);
  if(!['127.0.0.1','localhost','[::1]'].includes(base.hostname))return null;
  base.pathname=base.pathname.replace(/\/v1\/?$/,'').replace(/\/$/,'')+'/';
  const timeout=AbortSignal.timeout(4000);
  const options={method:'POST',redirect:'error',headers:{'Content-Type':'application/json'},signal:signal?AbortSignal.any([signal,timeout]):timeout};
  async function post(route,body){
    const response=await fetch(new URL(route,base),{...options,body:JSON.stringify(body)});
    if(!response.ok)throw Error('Tokenizer unavailable');
    return response.json();
  }
  try{
    const {prompt}=await post('apply-template',{messages,...(tools.length?{tools}:{})});
    if(typeof prompt!=='string')return null;
    const {tokens}=await post('tokenize',{content:prompt,add_special:true,parse_special:true});
    return Array.isArray(tokens)?tokens.length:null;
  }catch(error){if(signal?.aborted)throw error;return null}
}

async function waitForLocalWeights(model,signal){
  if(model.tokenizer!=='llama.cpp'||model.runtime?.kind!=='command')return;
  const url=new URL(model.endpoint);
  if(!['127.0.0.1','localhost','[::1]'].includes(url.hostname))return;
  url.pathname=url.pathname.replace(/\/v1\/?$/,'').replace(/\/$/,'')+'/health';
  while(!signal.aborted){
    try{
      const response=await fetch(url,{redirect:'error',signal:AbortSignal.any([signal,AbortSignal.timeout(2500)])});
      await response.body?.cancel();
      if(response.ok||response.status===404)return;
      if(response.status!==503)throw Error('Local model health HTTP '+response.status);
    }catch(e){if(signal.aborted||!['TimeoutError','TypeError'].includes(e.name))throw e;}
    await delay(400,undefined,{signal});
  }
  signal.throwIfAborted();
}
export async function prepareChat(task,system,history,tools,signal){
  const fallback=()=>composeChat(task,system,history,tools);
  if(task.model.tokenizer!=='llama.cpp')return fallback();
  const full=composeChat(task,system,history,tools,true);
  const limit=full.contextUsage.limit-full.contextUsage.output_reserve;
  const prior=full.messages.slice(1,-1),latest=full.messages.at(-1);
  const suffix=first=>[full.messages[0],...prior.slice(first),latest];
  const count=messages=>templateCount(task.model,protocolMessages(task.model,messages,tools),task.model.tool_mode==='json'?[]:tools,signal);
  let n=await count(full.messages);
  if(n===null)return fallback();
  let first=0;
  if(n>limit){
    let left=1,right=prior.length;
    while(left<right){
      const mid=Math.floor((left+right)/2), size=await count(suffix(mid));
      if(size===null)return fallback();
      if(size>limit)left=mid+1;else right=mid;
    }
    first=Math.min(left,prior.length);
    // Most chat templates expect a user to start the retained history.
    while(first<prior.length&&prior[first].role!=='user')first++;
    full.messages=suffix(first);
    n=await count(full.messages);
    if(n===null)return fallback();
    if(n>limit&&full.sources.length)return prepareChat({...task,project_memory:[]},system,history,tools,signal);
    if(n>limit)throw Error(`Prompt requires ${n} tokens; ${limit} available after output reserve.`);
  }
  full.omitted+=first;
  full.contextUsage.prompt_tokens=n;
  full.contextUsage.counted_by='llama.cpp';
  full.contextUsage.estimated_tokens.history=Math.ceil(prior.slice(first).reduce((a,m)=>a+messageSize(m),0)/2);
  return full;
}
export async function streamChat(model, messages, tools, onText, signal) {
  const ollama = model.protocol === 'ollama';
  const jsonTools = model.tool_mode === 'json' && (tools.length > 0 || model.force_json_tools);
  messages=protocolMessages(model,messages,tools);
  const contextBudget=Math.max(1,(model.context_tokens||2048)-(model.max_output_tokens||512))*2;
  const schemaSize=!jsonTools&&tools.length?JSON.stringify(tools).length:0;
  const nativeTools=jsonTools?[]:tools;
  let measured=await templateCount(model,messages,nativeTools,signal);
  if(measured===null || measured>contextBudget/2){
    messages=fitEvidence(messages,contextBudget-schemaSize);
    if(measured!==null)measured=await templateCount(model,messages,nativeTools,signal);
  }
  if(measured!==null&&measured>contextBudget/2)throw Error(`Prompt requires ${measured} tokens; ${contextBudget/2} available.`);
  const characters=messages.reduce((n,m)=>n+messageSize(m),0)+schemaSize;
  if(measured===null&&characters>contextBudget)throw Error(`Messages, instructions and tool results exceed the context estimate (${characters} characters, limit ${contextBudget}). Increase context or start a shorter conversation.`);
  const base = model.endpoint.replace(/\/$/, '');
  const body = ollama ? {
    model: model.engine_model, messages, stream: true, think: model.thinking === undefined ? false : model.thinking, keep_alive: `${model.keep_alive_seconds ?? 600}s`,
    options: {num_ctx: model.context_tokens || 2048, num_predict: model.max_output_tokens || 512,
      temperature: model.temperature ?? 0.3, ...(Number.isInteger(model.gpu_layers) ? {num_gpu: model.gpu_layers} : {}),
      ...(model.cpu_threads ? {num_thread: model.cpu_threads} : {}),...generationOptions(model)},
  } : {model: model.engine_model, messages, stream: true, max_tokens: model.max_output_tokens || 512,
    temperature: model.temperature ?? 0.3, stream_options: {include_usage: true},...generationOptions(model)};
  if (tools.length && !jsonTools) body.tools = tools;
  if(jsonTools){if(ollama)body.format=tools.length?'json':{type:'object',properties:{answer:{type:'string'}},required:['answer'],additionalProperties:false};else body.response_format={type:'json_object'};}
  const response = await fetch(base + (ollama ? '/api/chat' : '/chat/completions'), {
    method: 'POST', redirect: 'error', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body), signal,
  });
  if (!response.ok) throw Error(`Model HTTP ${response.status}: ${(await response.text()).slice(0, 1500)}`);
  let text = '', buffer = '', finished = false, usage = {}, finishReason = '';
  const calls = new Map();
  const chunk = item => {
    if (item.error) throw Error(typeof item.error === 'string' ? item.error : JSON.stringify(item.error));
    const delta = ollama ? item.message : item.choices?.[0]?.delta;
    if (delta?.content) {
      text += delta.content;
      if (text.length > MAX_OUTPUT) throw Error('Model output exceeds 128 KiB');
      if(!jsonTools)onText(text);
    }
    for (const call of delta?.tool_calls || []) {
      const index = call.index ?? call.function?.index ?? calls.size;
      if (ollama) calls.set(index, {id: call.id || `call_${index}`, type: 'function', function: call.function});
      else {
        const old = calls.get(index) || {id: '', type: 'function', function: {name: '', arguments: ''}};
        old.id += call.id || ''; old.function.name += call.function?.name || '';
        old.function.arguments += call.function?.arguments || '';
        if (old.function.arguments.length > 32768) throw Error('Tool arguments exceed 32 KiB');
        calls.set(index, old);
      }
    }
    if (calls.size > 8) throw Error('Too many tool calls in one response');
    if (ollama && item.done) {
      finished = true; finishReason = item.done_reason || 'stop';
      usage = {input_tokens: item.prompt_eval_count, output_tokens: item.eval_count,
        tokens_per_second: item.eval_duration ? item.eval_count / (item.eval_duration / 1e9) : null,
        load_seconds: (item.load_duration || 0) / 1e9};
    }
    if (!ollama && item.choices?.[0]?.finish_reason) {finished = true; finishReason = item.choices[0].finish_reason;}
    if (!ollama && item.usage) usage = {input_tokens: item.usage.prompt_tokens, output_tokens: item.usage.completion_tokens};
  };
  const line = value => {
    const trimmed = value.trim();
    if (!trimmed) return;
    if (ollama) chunk(JSON.parse(trimmed));
    else if (trimmed.startsWith('data:')) {
      const data = trimmed.slice(5).trim();
      if (data !== '[DONE]') chunk(JSON.parse(data));
    }
  };
  const decoder = new TextDecoder();
  for await (const bytes of response.body) {
    buffer += decoder.decode(bytes, {stream: true});
    if (buffer.length > 1024 * 1024) throw Error('Model stream record exceeds 1 MiB');
    let newline;
    while ((newline = buffer.indexOf('\n')) >= 0) {line(buffer.slice(0, newline)); buffer = buffer.slice(newline + 1);}
  }
  buffer += decoder.decode(); if (buffer.trim()) line(buffer);
  if (!finished) throw Error('Model stream ended before completion');
  if(jsonTools){const action=JSON.parse(text.trim().replace(/^```(?:json)?\s*|\s*```$/g,''));
    if(typeof action.answer==='string'&&!action.tool){onText(action.answer);return {content:action.answer,tool_calls:[],usage,finish_reason:finishReason};}
    if(typeof action.tool!=='string'||!action.arguments||typeof action.arguments!=='object'||Array.isArray(action.arguments))throw Error('Invalid JSON tool action from the model');
    return {content:'',raw_content:text,tool_calls:[{id:'json_call',type:'function',function:{name:action.tool,arguments:action.arguments}}],usage,finish_reason:finishReason};
  }
  return {content: text, tool_calls: [...calls.values()], usage, finish_reason: finishReason};
}

export async function runAgent(inputPath) {
  const task = JSON.parse(fs.readFileSync(inputPath, 'utf8'));
  const root = path.dirname(path.dirname(inputPath));
  const token = fs.readFileSync(path.join(root, 'data/controller.token'), 'utf8').trim();
  const port = JSON.parse(fs.readFileSync(path.join(root, 'data/controller.endpoint.json'), 'utf8')).port;
  const controller = new AbortController();
  const timeoutSeconds=task.timeout_seconds ?? 1200;
  const remaining=task.deadline_at ? Math.max(1,Date.parse(task.deadline_at)-Date.now()) : timeoutSeconds*1000;
  const timeout = setTimeout(() => controller.abort(Error(`Task exceeded ${timeoutSeconds} seconds`)), remaining);
  const progressPath = path.join(root, 'jobs', `${task.id}-progress.json`);
  const state = {id: task.id, status: 'running', content: '', steps: [], started_at: new Date().toISOString()};
  let lastWrite = 0, writeTimer;
  function persist(force = false) {
    if (!force && Date.now() - lastWrite < 180) {if(!writeTimer)writeTimer=setTimeout(()=>persist(true),180-(Date.now()-lastWrite));return;}
    clearTimeout(writeTimer);writeTimer=undefined;
    state.updated_at = new Date().toISOString();
    writeProgress(progressPath,state); lastWrite = Date.now();
  }
  async function rpc(method, params = {}, terminalWrite = false) {
    const r = await fetch(`http://127.0.0.1:${port}/rpc`, {method: 'POST', redirect: 'error',
      headers: {'Content-Type': 'application/json', Authorization: `Bearer ${token}`},
      body: JSON.stringify({method, params}), signal: terminalWrite ? AbortSignal.timeout(3000) : controller.signal});
    const result = await r.json(); if (!result.ok) throw Error(result.error?.message || `Controller HTTP ${r.status}`);
    return result.data;
  }
  persist(true);
  try {
    const mapped = new Map(), definitions = [];
    if (task.use_tools) {
      const connections = await rpc('mcp.list');
      const selected=Array.isArray(task.tool_selection)?new Set(task.tool_selection.map(t=>JSON.stringify([t.connection_id,t.tool]))):null;
      for (const connection of connections.filter(c => c.enabled && c.project_ids.includes(task.project_id))) {
        for (const name of connection.allowed_tools) {
          if(selected&&!selected.has(JSON.stringify([connection.id,name])))continue;
          let schema;try {schema = await rpc('mcp.tool.get', {id: connection.id, tool: name});} catch {await rpc('mcp.probe',{id:connection.id});schema=await rpc('mcp.tool.get',{id:connection.id,tool:name});}
          const alias = `tool_${mapped.size}`;
          mapped.set(alias, {connection: connection.id, name});
          definitions.push({type: 'function', function: {name: alias,
            description: `${connection.name}: ${name}. ${(schema.description || '').slice(0, 800)}`,
            parameters: schema.inputSchema}});
          if (definitions.length > 24 || JSON.stringify(definitions).length > 24000)
            throw Error('Too many enabled tools for this task. Select fewer tools in Extensions.');
        }
      }
    }
    task.model.force_json_tools=!!task.use_tools&&definitions.length>0;
    if(task.use_tools&&Array.isArray(task.tool_selection)&&definitions.length!==new Set(task.tool_selection.map(t=>JSON.stringify([t.connection_id,t.tool]))).size)throw Error('Some selected tools are no longer enabled for this project. Select tools again.');
    const userMessages=task.messages.filter(m=>m.role==='user');
    const last=userMessages.at(-1)?.content||'';
    const previous=previousWebRequest(userMessages.slice(0,-1).map(m=>m.content||''));
    const lookup=webIntent(last,previous);
    const readURL=lookup?'':readIntentURL(last);
    const requiredTool=lookup?'web_search':'web_read';
    const requiredAlias=[...mapped].find(([,v])=>v.connection==='internet'&&v.name===requiredTool)?.[0];
    const selectedEngine=namedEngine(last);
    const engine=lookup&&!webIntent(last)&&selectedEngine==='auto'?namedEngine(previous):selectedEngine;
    const system = systemInstructions(task)+(lookup?'\nUse web_search first with a short topical query. Read sources before summarizing a site. Never invent search hits.':'');
    if(task.model.tokenizer==='llama.cpp'&&task.model.runtime){
      state.phase='loading_model'; persist(true); await waitForLocalWeights(task.model,controller.signal);
    }
    state.phase = 'attachments'; persist(true);
    const prepared=await prepareChat(task,system,await prepareMessages(task,{deferBudget:true}),definitions,controller.signal);
    const messages=prepared.messages;
    state.omitted_messages=prepared.omitted;
    state.context_usage=prepared.contextUsage;
    const maxCalls = task.max_tool_calls ?? 4;
    async function finish(content,usage={},finish_reason='stop'){
      content=linkCitations(content,prepared.sources,task.language==='en');
      await rpc('run.finish',{id:task.id,content,usage,steps:state.steps,finish_reason,memory_sources:prepared.sources,omitted_messages:prepared.omitted,context_usage:prepared.contextUsage});
      Object.assign(state,{content,usage,finish_reason,status:'completed',phase:'done'});persist(true);return state;
    }
    if((lookup||readURL)&&(!requiredAlias||maxCalls<1))return await finish(task.language==='en'?'Enable Internet · Web and select '+requiredTool+' for this task.':'Включите Internet · Web и выберите '+requiredTool+' в инструментах чата.');
    let count = 0;
    async function callTool(target,args){
      const step = {connection_id: target.connection, tool: target.name, arguments: args, status: 'running', at: new Date().toISOString()};
      state.steps.push(step); state.phase = 'tool'; persist(true); count++;
      let result;
      try {result = await rpc('mcp.call', {id: target.connection, tool: target.name, project_id: task.project_id, run_id:task.id, arguments: args}); step.status = result?.isError ? 'failed' : 'completed';}
      catch (error) {result = {error: error.message}; step.status = 'failed';}
      if(controller.signal.aborted)throw controller.signal.reason;
      const serialized = JSON.stringify(result);
      step.result = serialized.length > 8000 ? serialized.slice(0, 8000) + '\n[Result truncated at 8000 characters]' : serialized;
      return {step,result};
    }
    for (let round = 0; round <= maxCalls; round++) {
      state.phase = 'generating'; state.content = ''; persist(true);
      const firstWeb=round===0&&Boolean(lookup||readURL);
      const available=firstWeb?definitions.filter(d=>d.function.name===requiredAlias):count<maxCalls?definitions:[];
      const reply=await streamChat(task.model,messages,available,text=>{if(!firstWeb){state.content=text;persist()}},controller.signal);
      if(firstWeb&&(!reply.tool_calls.length||reply.tool_calls.some(c=>c.function.name!==requiredAlias))){
        const args=readURL?{url:readURL}:{query:(webIntent(last)?last:previous||last).trim().slice(0,500)};
        reply.content='';reply.raw_content=JSON.stringify({tool:requiredAlias,arguments:args});
        reply.tool_calls=[{id:'requested_web_tool',type:'function',function:{name:requiredAlias,arguments:JSON.stringify(args)}}];
      }
      state.usage = reply.usage; state.finish_reason = reply.finish_reason;
      if (!reply.tool_calls.length) {
        if (!reply.content.trim()) throw Error('The model returned an empty answer');
        return await finish(reply.content,reply.usage,reply.finish_reason);
      }
      if (count + reply.tool_calls.length > maxCalls) throw Error(`Tool call limit reached (${maxCalls})`);
      messages.push(task.model.tool_mode==='json'?{role:'assistant',content:reply.raw_content}:{role: 'assistant', content: reply.content, tool_calls: reply.tool_calls});
      for (const call of reply.tool_calls) {
        const target = mapped.get(call.function?.name);
        if (!target) throw Error('Model requested an unavailable tool');
        const args = typeof call.function.arguments === 'string' ? JSON.parse(call.function.arguments) : call.function.arguments;
        if (!args || typeof args !== 'object' || Array.isArray(args)) throw Error('Tool arguments must be an object');
        const webSearch=target.connection==='internet'&&target.name==='web_search';
        if(webSearch&&(firstWeb||engine!=='auto'))args.engine=engine;
        if(firstWeb&&readURL)args.url=readURL;
        if(webSearch&&firstWeb&&!args.query&&!args.model_id)args.query=(webIntent(last)?last:previous||last).trim().slice(0,500);
        let {step,result}=await callTool(target,args);
        if(target.connection==='internet'&&target.name==='web_read'&&step.status==='failed'&&
           /MCP error -32000: Connection closed/i.test(result.error||'')&&count<maxCalls&&reply.tool_calls.length===1){
          ({step,result}=await callTool(target,args));
        }
        if(firstWeb&&step.status==='failed'){
          const error=result.error||result.content?.[0]?.text||'MCP error';
          return await finish((task.language==='en'?'The web request failed. ':'Не удалось выполнить запрос в интернет. ')+error,reply.usage);
        }
        if(webSearch&&step.status==='completed'){
          let payload=JSON.parse(result.content[0].text);
          if(task.desktop_browser&&(payload.attempts||[payload]).some(a=>a.search_url&&['unavailable','browser_required'].includes(a.status))){
            try{
              payload=await resolveBrowserSearch(payload,args.engine||'auto',async request=>{
                state.phase='browser_search';state.browser_search={...request,id:task.id,chat_id:task.chat_id};persist(true);
                for(;;){
                  await delay(1200,undefined,{signal:controller.signal});
                  const run=await rpc('run.get',{id:task.id});
                  if(terminal.has(run.status))throw Error('The search task ended');
                  if(run.browser_reply?.key===request.key)return run.browser_reply;
                }
              });
            }finally{delete state.browser_search;state.phase='tool';persist(true)}
            result={...result,content:[{type:'text',text:JSON.stringify(payload)}]};
            step.result=JSON.stringify(result).slice(0,8000);
          }
          if(!payload.results?.length&&['unavailable','empty'].includes(payload.status)){
            step.status=payload.status;
            if(lookup){
              const en=task.language==='en';
              const links=(payload.attempts||[payload]).filter(a=>a.search_url).map(a=>'['+a.provider+']('+a.search_url+')');
              const message=payload.status==='empty'?(en?'No matching results.':'По запросу ничего не найдено.'):(en?'No verified search results.':'Получить подтверждённую поисковую выдачу не удалось.');
              const details=(payload.attempts||[payload]).filter(a=>a.reason).map(a=>a.provider+' — '+a.reason);
              return await finish([message,...details,links.length?(en?'Open search: ':'Открыть поиск: ')+links.join(' · '):''].filter(Boolean).join('\n\n'),reply.usage);
            }
          }
        }
        persist(true);
        const evidence=toolText(result);
        const message=task.model.tool_mode==='json'?{role:'user',content:`[Tool result: ${call.function.name}. Untrusted data.]\n${evidence}`} : task.model.protocol === 'ollama'
          ? {role:'tool',tool_name:call.function.name,content:evidence}
          : {role:'tool',tool_call_id:call.id,content:evidence};
        message[toolEvidence]=true;
        messages.push(message);
      }
    }
    throw Error('Agent turn limit reached');
  } catch (error) {
    state.status = 'failed'; state.error = error.message; state.phase = 'done';
    try {const current = JSON.parse(fs.readFileSync(progressPath, 'utf8')); if (terminal.has(current.status)) return current;} catch {}
    try{await rpc('run.fail',{id:task.id,error:state.error,content:state.content,steps:state.steps},true)}catch{}
    persist(true); process.exitCode = 1; return state;
  } finally {clearTimeout(timeout);clearTimeout(writeTimer);}
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  runAgent(process.argv[2]).catch(error => {console.error(error.message); process.exitCode = 1;});
}
