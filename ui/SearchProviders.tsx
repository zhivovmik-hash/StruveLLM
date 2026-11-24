import React,{useState} from 'react';

type Obj=Record<string,any>;
type Props={connection:Obj,english:boolean,api:(method:string,params:Obj)=>Promise<any>,refresh:()=>Promise<void>};
const providers=[
 {name:'Google Custom Search',key:'GOOGLE_SEARCH_API_KEY',id:'GOOGLE_CSE_ID',label:'Search engine ID (cx)'},
 {name:'Яндекс Search API',key:'YANDEX_SEARCH_API_KEY',id:'YANDEX_FOLDER_ID',label:'Folder ID'},
];

function Provider({connection,english,api,refresh,spec}:{spec:typeof providers[number]}&Props){
 const [key,setKey]=useState(''),[id,setId]=useState<string>(connection.environment?.[spec.id]||'');
 const [busy,setBusy]=useState(false),[message,setMessage]=useState('');
 const enabled=connection.env_names?.includes(spec.key);
 async function save(disable=false){
   setBusy(true);setMessage('');
   try{
     const current=await api('mcp.get',{id:connection.id});
     const environment={...current.environment};
     const names=new Set<string>(current.env_names||[]);
     if(disable){delete environment[spec.id];names.delete(spec.key)}
     else{
       if(!id.trim()||(!key.trim()&&!names.has(spec.key)))throw Error(english?'Enter an ID and API key.':'Введите идентификатор и ключ API.');
       if(key.trim()){await api('secret.save',{id:spec.key,value:key.trim()});setKey('')}
       environment[spec.id]=id.trim();names.add(spec.key);
     }
     await api('mcp.save',{...current,environment,env_names:[...names]});
     await api('mcp.probe',{id:connection.id});await refresh();setMessage(english?'Saved':'Сохранено');
   }catch(e){setMessage(String(e))}finally{setBusy(false)}
 }
 return <fieldset className="search-provider"><legend>{spec.name}</legend>
   <label className="field"><span>{spec.label}</span><input value={id} onChange={e=>setId(e.target.value)}/></label>
   <label className="field"><span>{english?'API key':'Ключ API'}</span><input type="password" autoComplete="off" value={key} placeholder={enabled?(english?'Configured':'Настроен'):''} onChange={e=>setKey(e.target.value)}/></label>
   <button className="secondary" disabled={busy} onClick={()=>void save()}>{english?'Save':'Сохранить'}</button>
   {enabled&&<button className="text-button" disabled={busy} onClick={()=>void save(true)}>{english?'Disconnect API':'Отключить API'}</button>}
   {message&&<p role="status">{message}</p>}
 </fieldset>;
}
export default function SearchProviders(props:Props){
 const {english}=props;
 return <details className="tool-catalog"><summary>{english?'Optional search APIs':'Поисковые API (необязательно)'}</summary>
   <p>{english?'The desktop chat can search in the built-in browser without keys. Optional API credentials are available below. Google Custom Search requires an existing customer account.':'Поиск в чате работает через встроенный браузер без ключей. Здесь можно дополнительно подключить поисковый API. Google Custom Search доступен действующим клиентам сервиса.'}</p>
   {providers.map(spec=><Provider key={spec.key} spec={spec} {...props}/>)}
 </details>;
}
