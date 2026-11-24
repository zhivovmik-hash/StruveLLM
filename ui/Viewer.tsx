import React,{useEffect,useRef,useState} from 'react';
import SearchControls,{type BrowserRequest} from './BrowserSearch';
import Markdown,{SourceCode} from './Markdown';
import Spreadsheet from './Spreadsheet';
import Document from './Document';
import {invoke} from '@tauri-apps/api/core';
import {listen} from '@tauri-apps/api/event';
import {X,Globe,FolderOpen,ChevronLeft,ChevronRight,RefreshCw,PanelRightClose,FileText,Image,Code,Plus} from 'lucide-react';
type Tab={id:string,name:string,kind:string,url?:string,path?:string,content?:string,html?:string,warnings?:string[],format?:string,ready?:boolean,loaded?:number,sheets?:any[],sheets_truncated?:boolean,pending?:boolean,restoreError?:string};
const native=(id:string,operation:string,extra:any={})=>invoke('browser_tab',{id,operation,url:null,bounds:null,...extra});
export default function Viewer({open,onClose,lang,blocked,request}:{request:BrowserRequest|null,open:boolean,onClose:()=>void,lang:string,blocked:boolean}){
 const L=(ru:string,en:string)=>lang==='en'?en:ru;
 const [viewportWidth,setViewportWidth]=useState(window.innerWidth);
 useEffect(()=>{const resized=()=>setViewportWidth(window.innerWidth);window.addEventListener('resize',resized);return()=>window.removeEventListener('resize',resized)},[]);
 const panelWidth=(requested:number)=>Math.max(330,Math.min(requested,viewportWidth*0.55,viewportWidth-850));
 const [tabs,setTabs]=useState<Tab[]>([]),[active,setActive]=useState(''),[width,setWidth]=useState(Number(localStorage.getItem('viewer-width'))||480),[address,setAddress]=useState(''),[error,setError]=useState(''),[wrap,setWrap]=useState(false),[zoom,setZoom]=useState(100),[sourceView,setSourceView]=useState(false),[theme,setTheme]=useState(document.documentElement.dataset.theme||'dark');
 useEffect(()=>{const update=()=>setTheme(document.documentElement.dataset.theme||'dark');window.addEventListener('struve-theme',update);return()=>window.removeEventListener('struve-theme',update)},[]);
 const host=useRef<HTMLDivElement>(null),selected=tabs.find(t=>t.id===active);
 const current=selected?.pending||selected?.restoreError?undefined:selected;
 const restoring=useRef(new Set<string>());const tabsRef=useRef(tabs);tabsRef.current=tabs;
 const [restored,setRestored]=useState(false);
 const tabList=useRef<HTMLDivElement>(null);
 useEffect(()=>{tabList.current?.querySelector('[aria-selected="true"]')?.parentElement?.scrollIntoView({block:'nearest',inline:'nearest'})},[active]);
 const isBrowser=(t?:Tab)=>t&&['browser','pdf'].includes(t.kind);
 const report=(e:unknown)=>setError(String(e));
 const createView=(id:string,url:string)=>{const r=host.current?.getBoundingClientRect();return native(id,'create',{url,bounds:{width:r?.width||Math.max(width,330),height:r?.height||720}})};
 useEffect(()=>{
  let saved:Tab[]=[];
  try{const items=JSON.parse(localStorage.getItem('viewer-tabs')||'[]');if(Array.isArray(items))saved=items.slice(0,20)}catch{}
  const placeholders=saved.filter(t=>t&&typeof t.name==='string'&&(t.kind==='browser'||typeof t.path==='string')).map(t=>({...t,id:'viewer-'+crypto.randomUUID(),pending:true}));
  const index=Number(localStorage.getItem('viewer-active-index')||0);
  setTabs(placeholders);setActive(placeholders[index]?.id||placeholders[0]?.id||'');setRestored(true);
  return ()=>restoring.current.clear();
 },[]);
 useEffect(()=>{
  if(!open||blocked||!selected?.pending||restoring.current.has(selected.id))return;
  const {id}=selected;
  restoring.current.add(id);
  void (async()=>{
   try{
    const file:Tab=selected.kind==='browser'?{...selected}:{...await invoke<Tab>('preview_path',{path:selected.path}),id};
    if(isBrowser(file)){await createView(id,file.url||'about:blank');file.ready=true}
    if(!restoring.current.has(id)){if(file.ready)await native(id,'close');return}
    setTabs(ts=>ts.map(t=>t.id===id?{...t,...file,pending:false,restoreError:undefined}:t));
   }catch(e){setTabs(ts=>ts.map(t=>t.id===id?{...t,pending:false,restoreError:String(e)}:t))}
   finally{restoring.current.delete(id)}
  })();
 },[active,selected?.pending,open,blocked]);
 function retry(id:string){setTabs(ts=>ts.map(t=>t.id===id?{...t,pending:true,restoreError:undefined}:t))}

 useEffect(()=>{if(restored){localStorage.setItem('viewer-tabs',JSON.stringify(tabs.filter(t=>!t.id.startsWith('viewer-search-')).map(({name,kind,path,url})=>({name,kind,path,...(kind==='browser'?{url}:{})}))));localStorage.setItem('viewer-active-index',String(Math.max(0,tabs.findIndex(t=>t.id===active))))}},[tabs,active,restored]);
 useEffect(()=>{const pending=listen<{id:string,url:string}>('viewer-location',e=>{setTabs(ts=>ts.map(t=>t.id===e.payload.id?{...t,url:e.payload.url,...(t.kind==='browser'?{name:new URL(e.payload.url).hostname||L('Браузер','Browser')}:{})}:t))});return()=>{void pending.then(f=>f())}},[]);
 useEffect(()=>{const pending=listen<{id:string}>('viewer-loaded',e=>setTabs(ts=>ts.map(t=>t.id===e.payload.id?{...t,loaded:Date.now()}:t)));return()=>{void pending.then(f=>f())}},[]);
 useEffect(()=>{const openUrl=(e:Event)=>{void addBrowser((e as CustomEvent).detail)};window.addEventListener('struve-open-url',openUrl);return()=>window.removeEventListener('struve-open-url',openUrl)},[lang,restored,tabs.length]);
 useEffect(()=>{const openFile=(e:Event)=>{const path=(e as CustomEvent).detail?.path;if(typeof path==='string')void addPath(path)};window.addEventListener('struve-open-file',openFile);return()=>window.removeEventListener('struve-open-file',openFile)},[lang,restored,tabs.length]);
 useEffect(()=>{setAddress(current?.url||'')},[active,current?.url]);
 useEffect(()=>{
  let live=true;const sync=async()=>{for(const t of tabsRef.current.filter(t=>isBrowser(t)&&t.ready)){if(!live)return;try{if(open&&!blocked&&t.id===active&&host.current){const r=host.current.getBoundingClientRect();await native(t.id,'show',{theme,bounds:{x:r.x,y:r.y,width:r.width,height:r.height}})}else await native(t.id,'hide')}catch(e){if(live)report(e)}}};
  const observer=new ResizeObserver(()=>{void sync()});if(host.current)observer.observe(host.current);window.addEventListener('resize',sync);void sync();return()=>{live=false;observer.disconnect();window.removeEventListener('resize',sync)};
 },[active,open,blocked,width,current?.ready,current?.loaded,tabs.length,theme]);
 async function addFile(){if(!restored||tabs.length>=20){setError(L('Можно открыть до 20 вкладок','Open up to 20 tabs'));return}setError('');try{for(const t of tabs.filter(t=>isBrowser(t)&&t.ready))await native(t.id,'hide');const file=await invoke<any>('preview_file');if(!file)return;const id='viewer-'+crypto.randomUUID();const tab={...file,id};if(file.kind==='pdf'){await createView(id,file.url);tab.ready=true;}setTabs(ts=>[...ts,tab]);setActive(id);setZoom(100);}catch(e){report(e)}finally{setWidth(w=>w+0.001)}}
 async function addPath(path:string){setError('');const existing=tabs.find(t=>t.path?.toLowerCase()===path.toLowerCase());if(existing){setActive(existing.id);if(existing.restoreError)retry(existing.id);return}if(!restored||tabs.length>=20){setError(L('Можно открыть до 20 вкладок','Open up to 20 tabs'));return}try{const file=await invoke<any>('preview_path',{path});const id='viewer-'+crypto.randomUUID(),tab={...file,id};if(file.kind==='pdf'){await createView(id,file.url);tab.ready=true}setTabs(ts=>[...ts,tab]);setActive(id);setZoom(100)}catch(e){report(e)}}
 async function addBrowser(initial='about:blank'){if(!restored||tabs.length>=20){setError(L('Можно открыть до 20 вкладок','Open up to 20 tabs'));return}setError('');const id='viewer-'+crypto.randomUUID();try{await createView(id,initial);setTabs(ts=>[...ts,{id,name:new URL(initial).hostname||L('Браузер','Browser'),kind:'browser',url:initial,ready:true}]);setActive(id)}catch(e){report(e)}}
 async function closeTab(tab:Tab){try{if(isBrowser(tab)&&tab.ready)await native(tab.id,'close');restoring.current.delete(tab.id);const next=tabsRef.current.filter(t=>t.id!==tab.id);setTabs(ts=>ts.filter(t=>t.id!==tab.id));if(active===tab.id)setActive(next.at(-1)?.id||'')}catch(e){report(e)}}
 async function go(e:React.FormEvent){e.preventDefault();if(!current)return;try{let url=address.trim();if(!/^[a-z]+:/i.test(url))url='https://'+url;await native(current.id,'navigate',{url});setTabs(ts=>ts.map(t=>t.id===current.id?{...t,url}:t));setError('')}catch(e){report(e)}}
 const searchId=request?'viewer-search-'+request.key:'';
 const openingSearch=useRef(new Set<string>()),latestSearch=useRef(searchId);
 latestSearch.current=searchId;
 async function openSearch(){
   if(!request||!restored)return;
   if(tabsRef.current.some(t=>t.id===searchId)){setActive(searchId);return}
   if(openingSearch.current.has(searchId))return;
   if(tabsRef.current.length>=20){setError(L('Закройте вкладку, чтобы открыть поиск.','Close a tab to open search.'));return}
   openingSearch.current.add(searchId);
   try{
     await createView(searchId,request.url);
     if(latestSearch.current!==searchId){await native(searchId,'close');return}
     setTabs(ts=>[...ts,{id:searchId,name:request.engine==='google'?'Google':'Yandex',kind:'browser',ready:true,url:request.url}]);
     setActive(searchId);setError('');
   }catch(e){report(e)}finally{openingSearch.current.delete(searchId)}
 }
 useEffect(()=>{if(restored&&request)void openSearch()},[searchId,restored]);
 function resize(e:React.PointerEvent){const begin=e.clientX,original=panelWidth(width);const handle=e.currentTarget;handle.setPointerCapture(e.pointerId);const move=(event:PointerEvent)=>setWidth(panelWidth(original+begin-event.clientX));const stop=()=>{handle.removeEventListener('pointermove',move as any);handle.removeEventListener('pointerup',stop);setWidth(w=>{localStorage.setItem('viewer-width',String(w));return w})};handle.addEventListener('pointermove',move as any);handle.addEventListener('pointerup',stop)}
 return <aside className="viewer-panel" aria-label={L('Панель просмотра','Preview panel')} style={{display:open?'flex':'none',width:panelWidth(width)}}>
  <div className="viewer-divider" role="separator" aria-label={L('Ширина панели','Panel width')} onPointerDown={resize}/>
  <div className="viewer-heading"><strong>{L('Просмотр','Preview')}</strong><button title={L('Открыть файл','Open file')} aria-label={L('Открыть файл','Open file')} onClick={addFile}><FolderOpen size={17}/></button><button title={L('Новая вкладка браузера','New browser tab')} aria-label={L('Новая вкладка браузера','New browser tab')} onClick={()=>void addBrowser()}><Globe size={17}/><Plus size={12}/></button><button title={L('Свернуть панель','Collapse panel')} aria-label={L('Свернуть панель','Collapse panel')} onClick={onClose}><PanelRightClose size={17}/></button></div>
  <div className="viewer-tabs" role="tablist" ref={tabList}>{tabs.map(t=><div className={t.id===active?'active':''} key={t.id}><button role="tab" title={t.path||t.url||t.name} aria-selected={t.id===active} onClick={()=>{setActive(t.id);setError('')}}>{t.kind==='browser'?<Globe size={14}/>:t.kind==='image'?<Image size={14}/>:t.kind==='code'?<Code size={14}/>:<FileText size={14}/>}<span>{t.name}</span></button><button aria-label={L('Закрыть вкладку ','Close tab ')+t.name} onClick={()=>void closeTab(t)}><X size={13}/></button></div>)}</div>
  {request&&<SearchControls key={request.key} request={request} active={active===searchId} ready={open&&!blocked&&!!current?.ready} openTab={()=>void openSearch()} en={lang==='en'}/>}
  {error&&<div className="viewer-error" role="alert">{error}</div>}
  {current?.kind==='browser'&&<form className="browser-bar" onSubmit={go}><button type="button" aria-label={L('Назад','Back')} onClick={()=>native(current.id,'back').catch(report)}><ChevronLeft size={16}/></button><button type="button" aria-label={L('Вперёд','Forward')} onClick={()=>native(current.id,'forward').catch(report)}><ChevronRight size={16}/></button><button type="button" aria-label={L('Перезагрузить страницу','Reload page')} onClick={()=>native(current.id,'reload').catch(report)}><RefreshCw size={15}/></button><input aria-label={L('Адрес страницы','Page address')} value={address} onChange={e=>setAddress(e.target.value)} spellCheck={false}/><button type="submit">{L('Перейти','Go')}</button></form>}
  {current&&current.kind!=='browser'&&<div className="viewer-filebar"><span title={current.path}>{current.path}</span>{['md','markdown'].includes(current.format||'')&&<button className="secondary" onClick={()=>setSourceView(!sourceView)}>{sourceView?L('Документ','Document'):L('Исходник','Source')}</button>}{current.kind==='image'?<select aria-label={L('Масштаб изображения','Image zoom')} value={zoom} onChange={e=>setZoom(Number(e.target.value))}>{[50,75,100,150,200].map(z=><option key={z} value={z}>{z}%</option>)}</select>:!['pdf','spreadsheet','video'].includes(current.kind)&&<label><input type="checkbox" checked={wrap} onChange={e=>setWrap(e.target.checked)}/>{L('Перенос','Wrap')}</label>}</div>}
  <div className="viewer-content" ref={host}>
   {!restored&&<div className="viewer-empty">{L('Восстанавливаем вкладки…','Restoring tabs…')}</div>}
   {selected?.pending&&<div className="viewer-empty" role="status">{L('Открываем файл или страницу…','Opening file or page…')}</div>}
   {selected?.restoreError&&<div className="viewer-empty"><strong>{selected.name}</strong><p role="alert">{selected.restoreError}</p><button className="secondary" onClick={()=>retry(selected.id)}>{L('Попробовать снова','Try again')}</button></div>}
   {restored&&!selected&&<div className="viewer-empty"><FileText size={34}/><h2>{L('Документы и инструменты рядом','Documents and tools alongside')}</h2><p>{L('Откройте документ, исходный код, изображение или вкладку браузера.','Open a document, source file, image or browser tab.')}</p><button className="secondary" onClick={addFile}>{L('Открыть файл…','Open file…')}</button><button className="secondary" onClick={()=>void addBrowser()}>{L('Открыть браузер','Open browser')}</button></div>}
   {current?.kind==='rich-document'&&<Document html={current.html||''} warnings={current.warnings} lang={lang}/>}
   {current?.kind==='spreadsheet'&&<Spreadsheet key={current.id} sheets={current.sheets||[]} lang={lang} truncated={current.sheets_truncated}/>}
   {current?.kind==='video'&&<div className="image-preview"><video src={current.url} controls style={{maxWidth:'100%',maxHeight:'100%'}}/></div>}
   {current?.kind==='image'&&<div className="image-preview"><img src={current.url} alt={current.name} style={{maxWidth:zoom===100?'100%':'none',width:zoom===100?'auto':`${zoom}%`,maxHeight:zoom===100?'100%':'none'}}/></div>}
   {current&&['code','document'].includes(current.kind)&&<div className={'text-preview '+(wrap?'wrap':'')}><div className="preview-note">{current.format==='docx'?L('DOCX · текст документа','DOCX · document text'):current.kind==='code'?L('Исходный код · только чтение','Source code · read only'):L('Документ · только чтение','Document · read only')}</div>{['md','markdown'].includes(current.format||'')&&!sourceView?<Markdown content={current.content||''} basePath={current.path}/>:<SourceCode content={current.content||''} format={current.format||''} wrap={wrap}/>}</div>}
  </div>
 </aside>
}
