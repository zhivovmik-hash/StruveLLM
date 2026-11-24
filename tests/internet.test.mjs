import test from 'node:test';
import assert from 'node:assert/strict';
import {pageText,publicAddress,readSite,publicResults,searchWeb} from '../bridge/internet.mjs';
import {webIntent,namedEngine,readIntentURL,previousWebRequest} from '../bridge/web-intent.mjs';
import {runAgent,resolveBrowserSearch} from '../bridge/agent.mjs';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

test('keyless desktop search waits for each named engine and labels a user-selected source',async t=>{
  const original={};
  for(const key of ['STRUVE_DESKTOP_SEARCH','GOOGLE_SEARCH_API_KEY','GOOGLE_CSE_ID','YANDEX_SEARCH_API_KEY','YANDEX_FOLDER_ID']){
    original[key]=process.env[key];delete process.env[key];
  }
  process.env.STRUVE_DESKTOP_SEARCH='1';
  t.after(()=>{for(const [key,v] of Object.entries(original)){if(v===undefined)delete process.env[key];else process.env[key]=v}});
  const request=await searchWeb({query:'Струве & Астро',engine:'both'});
  assert.equal(request.status,'browser_required');
  const calls=[];
  const result=await resolveBrowserSearch(request,'both',async r=>{
    calls.push(r);
    assert.equal(new URL(r.url).searchParams.get(r.engine==='google'?'q':'text'),request.query);
    return r.engine==='google'?{status:'skipped',results:[]}:{status:'selected',results:[{url:'https://example.org/',title:'Example'}]};
  });
  assert.deepEqual(calls.map(c=>c.engine),['google','yandex']);assert.notEqual(calls[0].key,calls[1].key);
  assert.equal(result.status,'ok');assert.deepEqual(result.attempts.map(a=>a.status),['skipped','selected']);
  assert.equal(result.results[0].via,'user_selected');
});
test('automatic browser fallback returns the first successful engine, respecting site filters',async()=>{
  const payload={query:'site:example.org nebula',attempts:['Google','Яндекс'].map(provider=>({provider,status:'unavailable',search_url:'https://ignored.example/'}))};
  let count=0;
  const answer=await resolveBrowserSearch(payload,'auto',async r=>{
    assert.equal(new URL(r.url).hostname,'www.google.com');count++;
    return {status:'ok',results:[{url:'https://example.net/',title:'wrong site'},{url:'https://docs.example.org/',title:'Documentation'}]};
  });
  assert.equal(count,1);assert.deepEqual(answer.results.map(r=>r.url),['https://docs.example.org/']);
});
test('explicit URL reads retry a closed MCP transport once within the task budget',async t=>{
  const dir=await fs.mkdtemp(path.join(os.tmpdir(),'struve-search-test-'));
  t.after(()=>fs.rm(dir,{recursive:true,force:true}));
  await Promise.all(['data','jobs'].map(sub=>fs.mkdir(path.join(dir,sub))));
  await fs.writeFile(path.join(dir,'data/controller.token'),'fixture');
  await fs.writeFile(path.join(dir,'data/controller.endpoint.json'),'{"port":9876}');
  let calls=0;
  t.mock.method(globalThis,'fetch',async(url,options)=>{
    const p=JSON.parse(options.body);
    if(String(url).endsWith('/api/chat'))return new Response(JSON.stringify({done:true,message:{content:'{"answer":"Response"}'}})+'\n');
    const ok=data=>Response.json({ok:true,data});
    switch(p.method){
      case 'mcp.list':return ok([{id:'internet',name:'Web',enabled:true,project_ids:['p'],allowed_tools:['web_read']}]);
      case 'mcp.tool.get':return ok({inputSchema:{type:'object'}});
      case 'mcp.call':
        assert.equal(p.params.arguments.url,'https://example.org/');
        if(++calls===1)return Response.json({ok:false,error:{message:'MCP error -32000: Connection closed'}});
        return ok({content:[{type:'text',text:JSON.stringify({url:'https://example.org/',text:'Example page'})}]});
      case 'run.finish':return ok({});
      default:throw Error(p.method);
    }
  });
  for(const budget of [1,2]){
    calls=0;
    const task={id:'read-'+budget,project_id:'p',use_tools:true,max_tool_calls:budget,language:'en',messages:[{role:'user',content:'Read https://example.org/'}],model:{protocol:'ollama',engine_model:'fixture',tool_mode:'json',endpoint:'http://localhost:9877',context_tokens:4096}};
    const file=path.join(dir,'jobs',task.id+'-input.json');await fs.writeFile(file,JSON.stringify(task));
    const result=await runAgent(file);assert.equal(result.status,'completed');assert.equal(calls,budget);
    assert.deepEqual(result.steps.map(s=>s.status),budget===1?['failed']:['failed','completed']);
  }
});

test('chat recognises explicit search requests and respects opt-out',()=>{
  const prompt='Найди в Google и Яндексе сайт проекта Струве-Астро';
  assert.ok(webIntent(prompt));assert.equal(namedEngine(prompt),'both');
  assert.ok(webIntent('Проверь сайт в интернете'));
  assert.ok(webIntent('Ищи лучше',prompt));
  assert.ok(webIntent('Нет, это другой сайт. Ты ошибся. Ищи лучше.',previousWebRequest([prompt,'Ты не тот сайт нашёл.'])));
  assert.equal(previousWebRequest([prompt,'Посчитай 2 + 2']),'');
  assert.equal(namedEngine('Find an observatory using Google'),'google');
  assert.equal(namedEngine('Поищи в Яндексе фильм про Google'),'yandex');
  assert.equal(webIntent('Не ищи в интернете, расскажи сам'),false);
  assert.equal(webIntent('Найди файл в проекте'),false);
  assert.equal(readIntentURL('Прочитай https://example.org/. Перескажи кратко.'),'https://example.org/');
  assert.equal(readIntentURL('Не открывай https://example.org/'), '');
});

test('ordinary result headings yield links and a challenge page yields none',()=>{
  assert.deepEqual(publicResults('google','<a href="/url?q=https%3A%2F%2Fexample.org%2F"><h3>Result</h3></a>'),[{title:'Result',url:'https://example.org/',snippet:''}]);
  const result='<h2><a href="https://example.org/path">Пример</a></h2>';
  assert.equal(publicResults('yandex',result)[0].title,'Пример');
  assert.deepEqual(publicResults('yandex','<title>Вы не робот?</title>'+result),[]);
});

test('named search routes to each official API and preserves individual outcomes',async t=>{
  const query='Струве & астро + 1';
  const env={GOOGLE_SEARCH_API_KEY:'test-google',GOOGLE_CSE_ID:'test-cx',YANDEX_SEARCH_API_KEY:'test-yandex',YANDEX_FOLDER_ID:'test-folder'};
  for(const [name,value] of Object.entries(env)){
    const original=process.env[name];process.env[name]=value;
    t.after(()=>{if(original===undefined)delete process.env[name];else process.env[name]=original});
  }
  const calls=[];
  let googleUnavailable=false;
  t.mock.method(globalThis,'fetch',async(input,options)=>{
    const u=new URL(input);calls.push(u.hostname);assert.equal(options.redirect,'error');
    if(u.hostname==='customsearch.googleapis.com'){
      assert.equal(u.searchParams.get('cx'),'test-cx');assert.equal(u.searchParams.get('q'),query);
      assert.equal(options.headers['X-Goog-Api-Key'],'test-google');assert.ok(!u.href.includes('test-google'));
      if(googleUnavailable)return new Response('',{status:429});
      return Response.json({items:[{title:'Google hit',link:'https://example.com/result',snippet:'Example'}]});
    }
    assert.equal(u.href,'https://searchapi.api.cloud.yandex.net/v2/web/search');assert.equal(options.method,'POST');
    assert.equal(options.headers.Authorization,'Api-Key test-yandex');
    const body=JSON.parse(options.body);assert.equal(body.query.queryText,query);assert.equal(body.folderId,'test-folder');
    return Response.json({rawData:Buffer.from('<yandexsearch><response><doc><url>https://example.org/</url><title>Яндекс</title><passage>Текст</passage></doc></response></yandexsearch>').toString('base64')});
  });
  const both=await searchWeb({query,engine:'both'});
  assert.deepEqual(both.results.map(r=>r.provider),['Google','Яндекс']);
  assert.deepEqual(both.attempts.map((a,i)=>new URL(a.search_url).searchParams.get(i?'text':'q')),[query,query]);
  assert.ok(!JSON.stringify(both).includes('test-google'));googleUnavailable=true;
  const partial=await searchWeb({query,engine:'both'});assert.equal(partial.status,'ok');
  assert.deepEqual(partial.attempts.map(a=>a.status),['unavailable','ok']);
  const only=await searchWeb({query,engine:'google'});assert.equal(only.status,'unavailable');
  assert.equal(calls.filter(c=>c==='searchapi.api.cloud.yandex.net').length,2);
});

test('web reading extracts visible text and keeps multiple articles',()=>{
  const p=pageText('<title>Новости</title><body><nav>menu</nav><article>Alpha <b>&amp;</b> Beta</article><article>Gamma</article><script>ignore</script><span hidden>secret</span></body>');
  assert.equal(p.title,'Новости');
  assert.equal(p.text,'Alpha & Beta\nGamma');
});
test('only globally reachable addresses qualify',()=>{
  for(const address of ['127.0.0.1','10.1.2.3','169.254.169.254','100.64.0.9','::1','fe80::1','fc00::1','::ffff:127.0.0.1','2001:db8::1','2002:7f00:1::','0.0.0.0'])
    assert.equal(publicAddress(address),false,address);
  assert.equal(publicAddress('1.1.1.1'),true);
  assert.equal(publicAddress('2606:4700:4700::1111'),true);
});
test('local routes, schemes and embedded credentials fail before fetching',async()=>{
  for(const url of ['file:///C:/tmp/file','http://127.0.0.1/','https://user:pass@example.com','http://[::1]/'])
    await assert.rejects(readSite({url}),/public/);
});
