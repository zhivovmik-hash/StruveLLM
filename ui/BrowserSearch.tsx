import React,{useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';

export type BrowserRequest={key:string,id:string,chat_id:string,query:string,url:string,engine:'google'|'yandex'};
type Row={url:string,title:string};
export default function SearchControls({request,active,ready,openTab,en}:{request:BrowserRequest,active:boolean,ready:boolean,openTab:()=>void,en:boolean}){
 const [state,setState]=useState('loading'),[error,setError]=useState('');
 const resolved=useRef(false);
 const id='viewer-search-'+request.key;
 async function submit(status:string,results:Row[]){
   const res=await invoke<{ok:boolean,error?:{message:string}}>('api',{method:'run.browser.reply',params:{id:request.id,chat_id:request.chat_id,key:request.key,status,results}});
   if(!res.ok)throw Error(res.error?.message||'Browser search ended');
   setState('done');
 }
 async function inspect(operation='search-results'){
   return await invoke<{state:string,results?:Row[]}>('browser_tab',{id,operation,url:request.url});
 }
 useEffect(()=>{
   if(!active||!ready||state==='done')return;
   let live=true,timer:ReturnType<typeof setTimeout>;
   const poll=async()=>{
     try{
       let page=await inspect();
       if(live&&page.state==='redirects'){
         if(resolved.current)page={state:'choose'};
         else {resolved.current=true;setState('resolving');page=await inspect('resolve-results')}
       }
       if(!live)return;
       if(page?.state==='results'||page?.state==='empty'){await submit(page.state==='results'?'ok':'empty',page.results||[]);return}
       setState(page?.state||'waiting');setError('');
     }catch(e){if(live)setError(String(e))}
     if(live)timer=setTimeout(poll,1500);
   };
   void poll();return()=>{live=false;clearTimeout(timer)};
 },[id,active,ready,state==='done']);
 const label=en?{
   resolving:'Following search result links…',choose:'Open a result and select the page, or retry the links.',verification:'Complete the verification in this tab. The chat will continue automatically.',source:'Use this page as the source for the chat?',done:'Sent to the chat.',loading:'Opening search…',waiting:'Waiting for search results…'
 }:{resolving:'Определяем адреса источников…',choose:'Откройте результат и выберите страницу или повторите чтение ссылок.',verification:'Пройдите проверку в этой вкладке. Чат продолжится автоматически.',source:'Передать эту страницу в чат как источник?',done:'Передано в чат.',loading:'Открываем поиск…',waiting:'Ожидаем поисковую выдачу…'};
 return <div className="browser-search-task"><strong>{request.engine==='google'?'Google':en?'Yandex':'Яндекс'} · {request.query}</strong>
 <span role="status">{active?(label as Record<string,string>)[state]:en?'Search is waiting in its tab.':'Поиск ожидает в своей вкладке.'}</span>
 {error&&<span role="alert">{error}</span>}
 <div>{!active&&<button className="secondary" onClick={openTab}>{en?'Show search':'Открыть поиск'}</button>}
 {active&&state==='source'&&<button className="primary" onClick={()=>void inspect('select-source').then(v=>{if(v.state!=='selected')throw Error(en?'Page changed':'Страница изменилась');return submit('selected',v.results||[])}).catch(e=>setError(String(e)))}>{en?'Use this page':'Выбрать эту страницу'}</button>}
 {active&&state==='choose'&&<button className="secondary" onClick={()=>{resolved.current=false;setState('waiting')}}>{en?'Retry links':'Повторить чтение ссылок'}</button>}
 {state!=='done'&&<button className="text-button" onClick={()=>void submit('skipped',[]).catch(e=>setError(String(e)))}>{en?'Skip this engine':'Пропустить поисковик'}</button>}</div></div>
}
