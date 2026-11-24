import React,{useState} from 'react';

type Obj=Record<string,any>;
export default function Processes({instances,english,busy,onLogs,onStop}:{instances:Obj[],english:boolean,busy:boolean,onLogs:(id:string)=>void,onStop:(id:string)=>void}){
 const [shown,setShown]=useState(20);
 const L=(ru:string,en:string)=>english?en:ru;
 const active=(item:Obj)=>['running','starting'].includes(item.status);
 const running=instances.filter(active);
 const history=instances.filter(item=>!active(item)).sort((a,b)=>String(b.created_at||'').localeCompare(String(a.created_at||'')));
 const gib=(bytes:any)=>typeof bytes==='number'?(bytes/1073741824).toLocaleString(english?'en-US':'ru-RU',{maximumFractionDigits:2}):'—';
 const row=(item:Obj)=><div className="profile-row" key={item.id}>
  <span className={`badge ${active(item)?'green':item.status==='failed'?'red':'muted'}`}>{({running:L('Работает','Running'),starting:L('Запускается','Starting'),completed:L('Завершён','Completed'),failed:L('Ошибка','Failed'),stopped:L('Остановлен','Stopped'),interrupted:L('Прерван','Interrupted')} as Obj)[item.status]||item.status}</span>
  <div className="profile-name"><strong title={item.name}>{item.name}</strong><small>{item.profile?.name} · PID {item.pid||'—'}</small>
   {item.usage&&<small title={L('Общий предел выделенной памяти дерева процессов. Резидентная память показана отдельно.','Total committed memory limit for the process tree. Resident memory is shown separately.')}>
    {L('Выделено','Committed')} {gib(item.usage.committed_bytes)} / {item.profile?.ram_gib??'—'} GiB · {L('в RAM','resident')} {gib(item.usage.resident_bytes)} GiB
   </small>}
   {item.created_at&&<small>{new Date(item.created_at).toLocaleString(english?'en-GB':'ru-RU')}</small>}
  </div>
  <button className="secondary" disabled={busy} onClick={()=>onLogs(item.id)}>{L('Лог','Log')}</button>
  {active(item)&&<button className="secondary" disabled={busy} onClick={()=>onStop(item.id)}>{L('Остановить','Stop')}</button>}
 </div>;
 return <>
  <div className="profile-list">{running.length?running.map(row):<div className="inline-note" style={{padding:'0 18px'}}>{L('Нет активных процессов. Запуск и остановка доступны через окно и CLI.','No active processes. Start and stop processes through the window or CLI.')}</div>}</div>
  {!!history.length&&<details className="process-history"><summary>{L('История процессов','Process history')} · {history.length}</summary>
   <div className="profile-list">{history.slice(0,shown).map(row)}</div>
   {shown<history.length&&<button className="secondary" onClick={()=>setShown(value=>value+20)}>{L('Показать ещё','Show more')} · {Math.min(20,history.length-shown)}</button>}
  </details>}
 </>;
}
