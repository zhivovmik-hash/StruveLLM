import React,{useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
type Obj=Record<string,any>;
type Api=(method:string,params:Obj)=>Promise<any>;

export default function LibrarySources({project,api,en,openSource}:{project:string,api:Api,en:boolean,openSource:(href:string)=>void}){
 const L=(ru:string,english:string)=>en?english:ru;
 const [files,setFiles]=useState<Obj[]>([]),[query,setQuery]=useState(''),[hits,setHits]=useState<Obj[]>([]),[error,setError]=useState(''),[busy,setBusy]=useState(false);
 const load=async()=>setFiles(await api('library.list',{project_id:project}));
 const [semantic,setSemantic]=useState<Obj>({});
 const semanticStatus=()=>api('library.semantic_status',{project_id:project}).then(setSemantic);
 useEffect(()=>{void semanticStatus().catch(e=>setError(String(e)))},[project,files]);
 useEffect(()=>{void load().catch(e=>setError(String(e)))},[project]);
 useEffect(()=>{let live=true;if(!query.trim()){setHits([]);return}
   void api('library.search',{project_id:project,query:query.trim()}).then(rows=>{if(live)setHits(rows)}).catch(e=>{if(live)setError(String(e))});return()=>{live=false};
 },[query,project,files]);
 async function change(method:string,params:Obj){
   setError('');setBusy(true);
   try{const result=await api(method,{...params,project_id:project});if(Array.isArray(result)){const failed=result.filter(r=>r.error||r.unavailable);if(failed.length)setError(failed.map(r=>r.path+' · '+(r.error||L('недоступен','unavailable'))).join('\n'))}await load()}
   catch(e){setError(String(e))}finally{setBusy(false)}
 }
 async function add(graph:boolean){
   try{const file=await invoke<Obj|null>('choose_library_file',{graph});if(file)await change('library.index',{file:file.path,kind:graph?'graph':'text'})}
   catch(e){setError(String(e))}
 }
 return <section><p>{L('Выберите документы и исходники из папки проекта. Изменившиеся файлы переиндексируются во время простоя.','Choose documents and source files inside the project folder. Changed files are reindexed while idle.')}</p>
   <button disabled={busy} onClick={()=>void add(false)}>{L('Добавить файл','Add file')}</button>
   <button disabled={busy} onClick={()=>void add(true)}>{L('Импорт графа Graphify','Import Graphify graph')}</button>
   <button disabled={busy} onClick={()=>void change('library.refresh',{})}>{L('Обновить индекс','Refresh index')}</button>
   <details><summary>{L('Семантический поиск','Semantic search')}</summary>
     <p>{L('Локальный поиск по скрытым темам (LSI) на CPU. Для 3–1200 фрагментов. После изменения источников индекс можно пересобрать.','Local CPU topic search (LSI) for 3–1200 fragments. Rebuild after changing sources.')}</p>
     {semantic.enabled&&<small>{semantic.fragments} · {semantic.current?L('актуален','current'):L('требует обновления','needs rebuilding')}</small>}
     <button disabled={busy} onClick={()=>void change('library.semantic',{enabled:true})}>{semantic.enabled?L('Пересобрать','Rebuild'):L('Включить','Enable')}</button>
     {semantic.enabled&&<button disabled={busy} onClick={()=>void change('library.semantic',{enabled:false})}>{L('Отключить','Disable')}</button>}
   </details>
   {error&&<p role="alert">{error}</p>}
   <input type="search" value={query} onChange={e=>setQuery(e.target.value)} aria-label={L('Искать в документах','Search indexed sources')} placeholder={L('Искать в документах…','Search indexed sources…')}/>
   {query.trim()?hits.length?hits.map(h=><article key={h.source}><strong>{h.label}</strong><p>{h.relative}{h.match==='semantic'&&L(' · по теме',' · related topic')}</p><button onClick={()=>openSource(h.source)}>{L('Показать фрагмент','Show excerpt')}</button></article>):<p>{L('Совпадений нет.','No matches.')}</p>:
   files.map(f=><article key={f.id}><strong>{f.path}</strong><p>{f.kind==='graph'?L('Узлов','Nodes'):L('Фрагментов','Fragments')} · {f.chunks}</p>
     <button disabled={busy} onClick={()=>void change('library.forget',{id:f.id})}>{L('Убрать из индекса','Remove from index')}</button></article>)}
 </section>
}
export function SourceExcerpt({source,en,close}:{source:Obj,en:boolean,close:()=>void}){
 const dialog=useRef<HTMLDialogElement>(null);useEffect(()=>{dialog.current?.showModal()},[]);
 return <dialog ref={dialog} className="project-memory" aria-label={en?'Source excerpt':'Фрагмент источника'} onCancel={e=>{e.preventDefault();close()}}>
  <header><h2>{source.label}</h2><button aria-label={en?'Close':'Закрыть'} onClick={close}>×</button></header>
  <small>{source.relative} · {source.kind==='graph'?(en?'node':'узел'):(en?'line':'строка')} {source.anchor}</small>
  <pre style={{whiteSpace:'pre-wrap'}}>{source.text}</pre><button onClick={()=>{close();window.dispatchEvent(new CustomEvent('struve-open-file',{detail:{path:source.path}}))}}>{en?'Open file':'Открыть файл'}</button>
 </dialog>
}
