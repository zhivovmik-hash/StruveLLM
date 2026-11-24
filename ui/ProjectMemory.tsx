import React,{useEffect,useRef,useState} from 'react';
import './project-memory.css';
import {invoke} from '@tauri-apps/api/core';
import LibrarySources from './LibrarySources';

export type Note={id:number,revision:number,project_id:string,text:string,chat_id:string,run_id:string,role:string,source:string,type?:string,label?:string,relative?:string};
export type SourceMessage={chat_id:string,run_id:string,role:string,text:string};
type Props={project:string,english:boolean,source?:SourceMessage,close:()=>void,openSource:(href:string)=>void,
  api:(method:string,params:Record<string,unknown>)=>Promise<any>};

export default function ProjectMemory({project,english,source,close,openSource,api}:Props){
  const L=(ru:string,en:string)=>english?en:ru;
  const dialog=useRef<HTMLDialogElement>(null);
  const [query,setQuery]=useState(''),[offset,setOffset]=useState(0),[version,reload]=useState(0);
  const [notes,setNotes]=useState<Note[]>([]),[editing,setEditing]=useState<Note|null>(null);
  const [text,setText]=useState(source?.text||''),[error,setError]=useState(''),[busy,setBusy]=useState(false);
  const [tab,setTab]=useState('notes'),[exportPath,setExportPath]=useState('');
  const [forget,setForget]=useState<number|null>(null);
  useEffect(()=>{dialog.current?.showModal()},[]);
  useEffect(()=>{
    if(source)return;
    let live=true;
    api(query.trim()?'memory.search':'memory.list',{project_id:project,query:query.trim(),offset,limit:21})
      .then(rows=>{if(live){setNotes(rows);setError('')}}).catch(e=>{if(live)setError(String(e))});
    return ()=>{live=false};
  },[project,source,query,offset,version]);
  async function change(method:string,params:Record<string,unknown>){
    setBusy(true);setError('');
    try{
      await api(method,{...params,project_id:project});
      if(source){close();return}
      setEditing(null);setForget(null);setOffset(0);reload(n=>n+1);
    }catch(e){setError(String(e))}finally{setBusy(false)}
  }
  const title=source?L('Запомнить в проекте','Remember in project'):L('Память проекта','Project memory');
  return <dialog ref={dialog} className="project-memory" aria-label={title} onCancel={e=>{e.preventDefault();close()}}>
    <header><h2>{title}</h2><button type="button" onClick={close} aria-label={L('Закрыть','Close')}>×</button></header>
    {error&&<p role="alert">{error}</p>}
    {!source&&!editing&&<nav><button aria-pressed={tab==='notes'} onClick={()=>setTab('notes')}>{L('Заметки','Notes')}</button><button aria-pressed={tab==='files'} onClick={()=>setTab('files')}>{L('Документы и графы','Documents and graphs')}</button></nav>}
    {tab==='files'&&!source&&!editing?<LibrarySources project={project} api={api} en={english} openSource={href=>{close();openSource(href)}}/>:source||editing?<form onSubmit={e=>{e.preventDefault();void (editing?
      change('memory.edit',{id:editing.id,revision:editing.revision,text}):
      change('memory.add',{chat_id:source!.chat_id,run_id:source!.run_id,role:source!.role,text}))}}>
      <label>{L('Решение или заметка','Decision or note')}<textarea autoFocus rows={6} value={text} onChange={e=>setText(e.target.value)}/></label>
      <small>{Array.from(text).length} / 2000 · {L('Ссылка на сообщение сохранится вместе с записью.','The source message stays linked to the note.')}</small>
      <footer><button type="button" onClick={()=>source?close():setEditing(null)}>{L('Отмена','Cancel')}</button>
      <button disabled={busy||!text.trim()||Array.from(text).length>2000}>{L('Сохранить','Save')}</button></footer>
    </form>:<>
      <input autoFocus type="search" aria-label={L('Поиск по памяти','Search memory')} placeholder={L('Найти в памяти проекта…','Search project memory…')} value={query} onChange={e=>{setQuery(e.target.value);setOffset(0)}}/>
      {!notes.length&&<p>{query?L('Совпадений нет.','No matching notes.'):L('Сохраните решение кнопкой «Запомнить» под сообщением. Оно станет доступно другим чатам проекта.','Choose Remember under a message to share a decision with other chats in this project.')}</p>}
      {notes.slice(0,20).map(note=><article key={note.id}><p>{note.text}</p>
        <button onClick={()=>{close();openSource(note.source)}}>{L('К сообщению','Source message')}</button>
        <button onClick={()=>{setText(note.text);setEditing(note);setForget(null)}}>{L('Исправить','Edit')}</button>
        {forget===note.id?<><button disabled={busy} onClick={()=>void change('memory.forget',{id:note.id,revision:note.revision})}>{L('Подтвердить удаление','Confirm removal')}</button><button onClick={()=>setForget(null)}>{L('Отмена','Cancel')}</button></>:
          <button onClick={()=>setForget(note.id)}>{L('Забыть','Forget')}</button>}
      </article>)}
      <button onClick={async()=>{try{const r=await api('memory.export',{project_id:project});const f=await invoke<{path:string}|null>('save_export',{name:'project-memory.md',content:r.markdown});if(f)setExportPath(f.path)}catch(e){setError(String(e))}}}>{L('Экспорт заметок в Markdown','Export notes as Markdown')}</button>
      {exportPath&&<small role="status">{exportPath}</small>}
      <footer>{offset>0&&<button onClick={()=>setOffset(n=>Math.max(0,n-20))}>{L('Назад','Previous')}</button>}
        {notes.length>20&&<button onClick={()=>setOffset(n=>n+20)}>{L('Далее','Next')}</button>}</footer>
    </>}
  </dialog>;
}
