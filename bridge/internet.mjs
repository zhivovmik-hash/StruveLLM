import dns from 'node:dns/promises';
import http from 'node:http';
import https from 'node:https';
import net from 'node:net';
import fs from 'node:fs/promises';
import path from 'node:path';
import {createHash, randomUUID} from 'node:crypto';
import {Transform} from 'node:stream';
import {pipeline} from 'node:stream/promises';
import {createGunzip,createInflate,createBrotliDecompress} from 'node:zlib';
import {pathToFileURL} from 'node:url';
import ipaddr from 'ipaddr.js';
import {parseDocument, DomUtils} from 'htmlparser2';
import {McpServer} from '@modelcontextprotocol/sdk/server/mcp.js';
import {StdioServerTransport} from '@modelcontextprotocol/sdk/server/stdio.js';
import {z} from 'zod';

const agent='StruveLLM/1.0 (public web research)';
const MiB=1024*1024;
export function publicAddress(address){
  if(!ipaddr.isValid(address))return false;
  const ip=ipaddr.parse(address);
  if(ip.range()!=='unicast')return false;
  if(ip.kind()==='ipv6')return ip.match(ipaddr.parse('2000::'),3)&&
    !ip.match(ipaddr.parse('2001::'),23)&&!ip.match(ipaddr.parse('2002::'),16)&&!ip.match(ipaddr.parse('3fff::'),20);
  return !ip.match(ipaddr.parse('192.0.0.0'),24)&&!ip.match(ipaddr.parse('192.88.99.0'),24);
}
function webURL(input){
  const u=new URL(input);
  if(!['http:','https:'].includes(u.protocol)||u.username||u.password)throw Error('Use a public HTTP(S) URL without credentials');
  return u;
}
async function response(input,limit,signal){
  let url=webURL(input);
  for(let i=0;i<=5;i++){
    const host=url.hostname.replace(/^\[|\]$/g,'');
    const addresses=net.isIP(host)?[{address:host,family:net.isIP(host)}]:await dns.lookup(host,{all:true,verbatim:false});
    if(!addresses.length||addresses.some(a=>!publicAddress(a.address)))throw Error('Only public internet destinations are supported');
    const addr=addresses.find(a=>a.family===4)||addresses[0];
    const transport=url.protocol==='https:'?https:http;
    const res=await new Promise((resolve,reject)=>{
      const req=transport.get(url,{agent:false,signal,family:addr.family,lookup:(_host,opts,cb)=>opts.all?cb(null,[addr]):cb(null,addr.address,addr.family),
        headers:{'user-agent':agent,'accept-encoding':'identity','accept':'*/*'}},resolve);
      req.once('error',reject);
    });
    if([301,302,303,307,308].includes(res.statusCode)&&res.headers.location){
      res.destroy();url=webURL(new URL(res.headers.location,url));continue;
    }
    if(res.statusCode<200||res.statusCode>=300){res.destroy();throw Error('HTTP '+res.statusCode+' at '+url.hostname)}
    if(Number(res.headers['content-length'])>limit){res.destroy();throw Error('Response exceeds the requested byte limit')}
    return {res,url:url.href};
  }
  throw Error('Too many redirects');
}
async function readBytes(url,limit,signal){
  const r=await response(url,limit,signal);const chunks=[];let bytes=0;
  const encoding=String(r.res.headers['content-encoding']||'identity').toLowerCase();
  const decompress={gzip:createGunzip,deflate:createInflate,br:createBrotliDecompress}[encoding];
  if(encoding!=='identity'&&!decompress){r.res.destroy();throw Error('Unsupported HTTP content encoding')}
  try{
    const stream=decompress?r.res.pipe(decompress()):r.res;
    if(stream!==r.res)r.res.once('error',error=>stream.destroy(error));
    for await(const chunk of stream){bytes+=chunk.length;if(bytes>limit)throw Error('Page exceeds the byte limit');chunks.push(chunk)}
  }finally{r.res.destroy()}
  const type=String(r.res.headers['content-type']||'');
  let decoder;try{decoder=new TextDecoder(/charset=["']?([\w-]+)/i.exec(type)?.[1]||'utf-8')}catch{decoder=new TextDecoder()}
  return {url:r.url,type,body:decoder.decode(Buffer.concat(chunks))};
}
const elements=(doc,test)=>DomUtils.findAll(test,doc.children||[]);
const clean=t=>t.replace(/\s+/g,' ').trim();
function elementText(node){return clean(DomUtils.textContent(node))}
export function pageText(html){
  const doc=parseDocument(html);
  const title=elements(doc,e=>e.name==='title').map(elementText)[0]||'';
  for(const e of elements(doc,e=>['script','style','head','template','noscript','svg','nav'].includes(e.name)||e.attribs.hidden!==undefined))DomUtils.removeElement(e);
  const body=elements(doc,e=>e.name==='main')[0]||elements(doc,e=>e.name==='body')[0]||doc;
  const parts=[];
  function walk(n){
    if(n.type==='text'){parts.push(n.data.replace(/\s+/g,' '));return}
    const block=['p','li','br','section','article','tr','div','pre','h1','h2','h3','h4','blockquote'].includes(n.name);
    if(block)parts.push('\n');for(const child of n.children||[])walk(child);if(block)parts.push('\n');
  }
  walk(body);
  return {title,text:parts.join('').split(/\n+/).map(clean).filter(Boolean).join('\n')};
}
function hits(items,limit,site){
  const seen=new Set();return items.filter(r=>{
    try{r.url=webURL(r.url).href}catch{return false}
    const host=new URL(r.url).hostname.toLowerCase();
    if(site&&host!==site&&!host.endsWith('.'+site))return false;
    if(!r.title||seen.has(r.url))return false;seen.add(r.url);return true;
  }).slice(0,limit);
}
async function modelSearch(term,limit){
  const url=new URL('https://huggingface.co/api/models');
  url.search=new URLSearchParams({search:term,limit:String(limit),sort:'likes',direction:'-1'});
  const {body}=await readBytes(url,MiB,AbortSignal.timeout(12000));
  return JSON.parse(body).filter(m=>typeof m.id==='string').map(m=>({title:m.id,url:'https://huggingface.co/'+m.id.split('/').map(encodeURIComponent).join('/'),snippet:(m.tags||[]).filter(t=>!t.startsWith('region:')).slice(0,10).join(', ')}));
}
const engines={google:'Google',yandex:'Яндекс'};
function searchLink(engine,query){
  const url=new URL(engine==='google'?'https://www.google.com/search':'https://yandex.ru/search/');
  url.searchParams.set(engine==='google'?'q':'text',query);
  return url.href;
}
export function publicResults(engine,html){
  const doc=parseDocument(html);
  const title=elements(doc,n=>n.name==='title').map(elementText).join(' ');
  if(/не робот|вы робот|captcha|unusual traffic|проверка безопасности|are you a robot/i.test(title))return [];
  const rows=[];
  for(const heading of elements(doc,n=>n.name==='h3'||(engine==='yandex'&&n.name==='h2'))){
    let a=heading.parent;
    while(a&&a.name!=='a'&&a.name!=='body')a=a.parent;
    if(a?.name!=='a')a=elements(heading,n=>n.name==='a')[0];
    if(!a?.attribs.href)continue;
    const base=engine==='google'?'https://www.google.com/':'https://yandex.ru/';
    let url;try{url=new URL(a.attribs.href,base);
      if(engine==='google'&&url.origin===new URL(base).origin&&url.pathname==='/url')url=new URL(url.searchParams.get('q')||url.searchParams.get('url'));
      if(!/^https?:$/.test(url.protocol)||url.origin===new URL(base).origin)continue;
    }catch{continue}
    rows.push({title:elementText(heading),url:url.href,snippet:''});
  }
  return rows;
}
export function yandexRows(raw){
  const doc=parseDocument(Buffer.from(raw,'base64').toString('utf8'),{xmlMode:true});
  const error=elements(doc,n=>n.name==='error')[0];
  if(error){if(error.attribs.code==='15')return [];throw Error('Yandex Search API: '+elementText(error).slice(0,200))}
  if(!elements(doc,n=>n.name==='response').length)throw Error('Unexpected Yandex Search API document');
  return elements(doc,n=>n.name==='doc').map(d=>{
    const field=name=>elements(d,n=>n.name===name).map(elementText).join(' ');
    return {title:field('title'),url:field('url'),snippet:field('passage')||field('headline')};
  });
}
// Only fixed vendor endpoints receive credentials; never follow redirects with an API key.
async function vendorJSON(url,options){
  const response=await fetch(url,{...options,redirect:'error',signal:AbortSignal.timeout(16000)});
  if(!response.ok){await response.body?.cancel();throw Error('Search API HTTP '+response.status)}
  const chunks=[];let bytes=0;
  for await(const chunk of response.body){bytes+=chunk.length;if(bytes>4*MiB)throw Error('Search API response too large');chunks.push(chunk)}
  return JSON.parse(Buffer.concat(chunks).toString('utf8'));
}
async function namedSearch(engine,query,limit,site){
  const provider=engines[engine],search_url=searchLink(engine,query),env=process.env;
  const base={provider,query,search_url};
  try{
    let rows;
    if(engine==='google'&&(env.GOOGLE_SEARCH_API_KEY||env.GOOGLE_CSE_ID)){
      if(!env.GOOGLE_SEARCH_API_KEY||!env.GOOGLE_CSE_ID)throw Error('Google requires GOOGLE_SEARCH_API_KEY and GOOGLE_CSE_ID');
      const url=new URL('https://customsearch.googleapis.com/customsearch/v1');
      url.search=new URLSearchParams({cx:env.GOOGLE_CSE_ID,q:query,num:String(limit)});
      const data=await vendorJSON(url,{headers:{'X-Goog-Api-Key':env.GOOGLE_SEARCH_API_KEY}});
      rows=(data.items||[]).map(r=>({title:r.title,url:r.link,snippet:r.snippet||''}));
    }else if(engine==='yandex'&&(env.YANDEX_SEARCH_API_KEY||env.YANDEX_FOLDER_ID)){
      if(!env.YANDEX_SEARCH_API_KEY||!env.YANDEX_FOLDER_ID)throw Error('Yandex requires YANDEX_SEARCH_API_KEY and YANDEX_FOLDER_ID');
      const data=await vendorJSON('https://searchapi.api.cloud.yandex.net/v2/web/search',{method:'POST',headers:{Authorization:'Api-Key '+env.YANDEX_SEARCH_API_KEY,'Content-Type':'application/json'},
        body:JSON.stringify({folderId:env.YANDEX_FOLDER_ID,query:{searchType:/[а-яё]/i.test(query)?'SEARCH_TYPE_RU':'SEARCH_TYPE_COM',queryText:query},groupSpec:{groupsOnPage:String(limit),docsInGroup:'1'},responseFormat:'FORMAT_XML'})});
      if(!data.rawData)throw Error('Yandex Search API returned no document');
      rows=yandexRows(data.rawData);
    }else if(env.STRUVE_DESKTOP_SEARCH==='1'){
      return {...base,status:'browser_required',results:[]};
    }else{
      const {body,url}=await readBytes(search_url,3*MiB,AbortSignal.timeout(9000));
      rows=publicResults(engine,body);
      if(!rows.length){
        if(/showcaptcha|\/sorry\/|\/challenge\//i.test(url)||/вы не робот|unusual traffic/i.test(body))throw Error('Поисковик запросил проверку пользователя.');
        throw Error('Публичная страница не предоставила результаты автоматическому клиенту.');
      }
    }
    const results=hits(rows,limit,site);
    return {...base,status:results.length?'ok':'empty',results};
  }catch(e){return {...base,status:'unavailable',reason:e.message,results:[]}}
}
export async function searchWeb({query,limit=5,model_id,engine='auto'}){
  query=query||model_id;
  if(!query)throw Error('Provide a query or a model_id');
  const site=query.match(/(?:^|\s)site:([\w.-]+)/i)?.[1]?.toLowerCase();
  if(engine==='both'){
    const answers=await Promise.all(['google','yandex'].map(name=>namedSearch(name,query,limit,site)));
    const results=answers.flatMap(a=>a.results.map(r=>({...r,provider:a.provider})));
    return {provider:'Google + Яндекс',query,status:results.length?'ok':answers.some(a=>a.status==='browser_required')?'browser_required':answers.some(a=>a.status==='unavailable')?'unavailable':'empty',
      attempts:answers.map(({provider,search_url,status,reason})=>({provider,search_url,status,reason})),results};
  }
  if(engine!=='auto')return namedSearch(engine,query,limit,site);
  if(model_id||site==='huggingface.co'||/hugging\s*face.*model card|model card.*hugging\s*face/i.test(query)){
    const words=query.replace(/site:[^\s]+/ig,'').replace(/\b(?:diffusers|hugging\s*face|example|documentation|model card|model)\b/ig,'').trim();
    const term=(model_id||words).trim().replace(/["']/g,'').replace(/\s+/g,'-');
    if(term && (!site || site==='huggingface.co')){
      const candidates=new Set([term,term.replace(/([a-z])(\d)/ig,'$1.$2')]);
      for(const candidate of candidates){
        let rows;
        try{rows=await modelSearch(candidate,limit)}catch(e){if(model_id)throw e;break}
        if(rows.length)return {provider:'Hugging Face Hub',query,results:rows};
      }
      if(model_id)return {provider:'Hugging Face Hub',query,results:[]};
    }
  }
  const attempts=[];
  for(const name of (/[а-яё]/i.test(query)?['yandex','google']:['google','yandex'])){
    const answer=await namedSearch(name,query,limit,site);
    if(answer.results.length)return answer;
    attempts.push({provider:answer.provider,search_url:answer.search_url,status:answer.status,reason:answer.reason||'No matching results'});
  }
  const ddg=new URL('https://html.duckduckgo.com/html/');ddg.searchParams.set('q',query);
  try{
    const {body}=await readBytes(ddg,2*MiB,AbortSignal.timeout(11000));const doc=parseDocument(body);
    const rows=elements(doc,e=>e.name==='a'&&(e.attribs.class||'').split(/\s+/).includes('result__a')).map(a=>{
      const target=new URL(a.attribs.href,ddg);
      let parent=a.parent;while(parent&&!(parent.attribs?.class||'').split(/\s+/).includes('result'))parent=parent.parent;
      const snippet=parent&&elements(parent,e=>(e.attribs.class||'').split(/\s+/).includes('result__snippet'))[0];
      return {title:elementText(a),url:target.searchParams.get('uddg')||target.href,snippet:snippet?elementText(snippet):''};
    });
    const results=hits(rows,limit,site);if(results.length)return {provider:'DuckDuckGo',query,results};
  }catch{/* Try the independent RSS search endpoint. */}
  const bing=new URL('https://www.bing.com/search');bing.searchParams.set('format','rss');bing.searchParams.set('q',query);
  try{
  const {body}=await readBytes(bing,2*MiB,AbortSignal.timeout(14000));const doc=parseDocument(body,{xmlMode:true});
  if(!elements(doc,e=>e.name==='channel').length)throw Error('Search provider did not return an RSS feed');
  const rows=elements(doc,e=>e.name==='item').map(item=>{
    const field=k=>elements(item,e=>e.name===k).map(elementText)[0]||'';
    return {title:field('title'),url:field('link'),snippet:field('description')};
  });
  const terms=query.toLowerCase().replace(/site:[^\s]+/g,'').match(/[\p{L}\p{N}]{3,}/gu)||[];
  const results=hits(rows,limit,site).filter(r=>!terms.length||terms.some(t=>(r.title+' '+r.url+' '+r.snippet).toLowerCase().includes(t)));
  if(results.length)return {provider:'Bing RSS',query,status:'ok',results};
  }catch{/* Return both original provider links even when the fallback is offline. */}
  return {provider:'Auto',query,status:'unavailable',reason:'Поисковики не вернули подходящих ссылок.',attempts,results:[]};
}
export async function readSite({url,start_character=0,max_characters=12000,find}){
  const r=await readBytes(url,5*MiB,AbortSignal.timeout(25000));
  if(!/^(text\/|application\/(xhtml\+xml|json|[\w.+-]*xml|javascript))/i.test(r.type))throw Error('Use web_download for binary documents');
  const {title,text}=r.type.toLowerCase().includes('html')?pageText(r.body):{title:'',text:r.body};
  if(find){
    const index=text.toLowerCase().indexOf(find.toLowerCase(),start_character);
    if(index>=0)start_character=Math.max(start_character,index-120);
    else return {url:r.url,title,text:'',found:false,next_character:null,total_characters:text.length};
  }
  return {url:r.url,title,content_type:r.type,text:text.slice(start_character,start_character+max_characters),total_characters:text.length,
    next_character:start_character+max_characters<text.length?start_character+max_characters:null};
}
export async function downloadFile({url,filename,max_mib=64},root=process.env.STRUVE_PROJECT_ROOT){
  if(!root)throw Error('Select a project before downloading');
  const base=await fs.realpath(root);let dir=base;
  for(const part of ['downloads','internet']){
    dir=path.join(dir,part);await fs.mkdir(dir).catch(e=>{if(e.code!=='EEXIST')throw e});
    if((await fs.lstat(dir)).isSymbolicLink()||await fs.realpath(dir)!==dir)throw Error('Downloads directory must be inside this project');
  }
  const r=await response(url,max_mib*MiB,AbortSignal.timeout(150000));
  let name=filename||new URL(r.url).pathname.split('/').pop()||'download';
  name=name.replace(/[^\p{L}\p{N}._ -]/gu,'_').slice(0,120).replace(/[ .]+$/,'')||'download';
  const id=randomUUID();const dest=path.join(dir,id+'-'+name);const tmp=path.join(dir,'.'+id+'.part');
  let size=0;const hash=createHash('sha256');
  try{
    const fd=await fs.open(tmp,'wx');
    await pipeline(r.res,new Transform({transform(chunk,_enc,cb){size+=chunk.length;
      if(size>max_mib*MiB){cb(Error('Download exceeds the byte limit'));return}hash.update(chunk);cb(null,chunk)}}),fd.createWriteStream());
    await fs.rename(tmp,dest);
    return {url:r.url,path:dest,project_path:path.relative(base,dest),bytes:size,sha256:hash.digest('hex'),content_type:r.res.headers['content-type']||''};
  }finally{r.res.destroy();await fs.rm(tmp,{force:true})}
}
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href){
  const server=new McpServer({name:'struve-internet',version:'1.0.0'});
  const wrap=handler=>async args=>{try{return {content:[{type:'text',text:JSON.stringify(await handler(args))}]}}catch(e){return {isError:true,content:[{type:'text',text:e.message}]}}};
  const url=z.string().url().max(8192);
  server.registerTool('web_search',{description:'Search the web. Choose engine google, yandex, or both when requested. If blocked use search_url. model_id searches Hugging Face in auto mode.',inputSchema:{query:z.string().trim().min(1).max(500).optional(),model_id:z.string().trim().min(1).max(180).optional(),engine:z.enum(['auto','google','yandex','both']).default('auto'),limit:z.number().int().min(1).max(10).default(5)},annotations:{readOnlyHint:true,openWorldHint:true}},wrap(searchWeb));
  server.registerTool('web_read',{description:'Read a public page. Optional find jumps to an excerpt containing a phrase; next_character continues reading.',inputSchema:{url,find:z.string().trim().min(1).max(160).optional(),start_character:z.number().int().nonnegative().default(0),max_characters:z.number().int().min(100).max(50000).default(12000)},annotations:{readOnlyHint:true,openWorldHint:true}},wrap(readSite));
  server.registerTool('web_download',{description:'Download a public file into downloads/internet of the selected project; returns the path, byte count and SHA-256.',inputSchema:{url,filename:z.string().min(1).max(120).optional(),max_mib:z.number().int().min(1).max(1024).default(64)},annotations:{readOnlyHint:false,destructiveHint:false,openWorldHint:true}},wrap(downloadFile));
  await server.connect(new StdioServerTransport());
}
