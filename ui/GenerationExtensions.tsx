import React,{useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {Check,Download,ExternalLink,FolderOpen,Search,RefreshCw,SlidersHorizontal,X} from 'lucide-react';
import './generation-extensions.css';

type Obj=Record<string,any>;
type Props={data:Obj,api:(m:string,p?:Obj)=>Promise<any>,refresh:()=>Promise<void>};
const families:Record<string,string>={sd15:'SD 1.5',sdxl:'SDXL',sd35:'SD 3.5',flux1:'FLUX.1','flux2-klein':'FLUX.2 Klein','minimax-h3':'H3 Video','h3-image':'H3 Image'};
const h3Templates:Record<string,[string,string]>={
 'MiniMax-H3-Image_Edit_Low_RAM':['Изменить изображение','Edit one image'],
 'MiniMax-H3-Image_Edit_Two_Pictures_Low_RAM':['Редактировать по двум изображениям','Edit with two images'],
 'MiniMax-H3-Image_T2I_Low_RAM':['Создать изображение по описанию','Generate an image from text'],
 'MiniMax-H3_Planning_and_Prompt_Lint':['Спланировать сцену и проверить промпт','Plan a scene and review its prompt'],
 'MiniMax-H3_AV_Latent_Tools':['Работа с латентами видео и звука','Video and audio latent tools'],
 'MiniMax-H3_AV_Advanced':['Настроить генерацию видео со звуком','Configure audio and video generation'],
 'MiniMax-H3_Audio_Reroll_Experimental':['Перегенерировать звук','Regenerate the audio']
};
export function compatibleExtensions(records:Obj[],model?:Obj){
 return model?records.filter(x=>x.family===model.architecture&&(!x.model_id||x.model_id===model.id)):[];
}
export default function GenerationExtensions({data,api,refresh}:Props){
 const en=data.settings.language==='en',L=(ru:string,eng:string)=>en?eng:ru;
 const [collection,setCollection]=useState<Obj>({catalog:[],installed:[]}),[query,setQuery]=useState(''),[family,setFamily]=useState(''),[section,setSection]=useState<'installed'|'available'>('installed');
 const [active,setActive]=useState<Obj|null>(null),[form,setForm]=useState<Obj>({}),[plan,setPlan]=useState<Obj|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState(''),[message,setMessage]=useState('');
 const [chosenModel,setChosenModel]=useState('');
 const [graph,setGraph]=useState(''),[deps,setDeps]=useState<Obj|null>(null);
 const [editing,setEditing]=useState<Obj|null>(null);
 const [comfy,setComfy]=useState<Obj|null>(null),[probe,setProbe]=useState<Obj|null>(null),[sample,setSample]=useState(''),[report,setReport]=useState('');
 const models:Obj[]=data.media_models||[],model=models.find(m=>m.id===chosenModel);
 const [market,setMarket]=useState<Obj>({items:[]}),[offset,setOffset]=useState(0),[epoch,setEpoch]=useState(0),[syncing,setSyncing]=useState(false),[syncError,setSyncError]=useState('');
 const refreshing=useRef(false);
 const reload=async()=>{setCollection(await api('media.extensions.catalog'));setEpoch(n=>n+1)};
 const sync=async(force:boolean)=>{if(refreshing.current)return;refreshing.current=true;setSyncing(true);setSyncError('');
   try{await api('media.market.refresh',{force});setEpoch(n=>n+1)}catch(e){setSyncError(String(e))}finally{refreshing.current=false;setSyncing(false)}};
 useEffect(()=>{void sync(false);const timer=setInterval(()=>void sync(false),86400000);return()=>clearInterval(timer)},[]);
 useEffect(()=>{let current=true;const timer=setTimeout(()=>{api('media.market.list',{section,query,family,offset,limit:24}).then(v=>{if(current)setMarket(v)}).catch(e=>{if(current)setError(String(e))})},query?180:0);
   return()=>{current=false;clearTimeout(timer)}},[section,query,family,offset,epoch]);
 useEffect(()=>{let live=true;api('media.extensions.catalog').then(v=>{if(live)setCollection(v)}).catch(e=>{if(live)setError(String(e))});return()=>{live=false}},[]);
 const open=(c:Obj)=>{setActive(c);setDeps(null);setGraph('');setPlan(null);setProbe(null);setReport('');setError('');setMessage('');setForm({kind:c.id,name:'',description:'',model_id:chosenModel||models[0]?.id||'',path:'',strength:1})};
 const set=(key:string,v:any)=>{setForm(old=>({...old,[key]:v}));setPlan(null)};
 const choose=async()=>{try{const f=await invoke<Obj|null>('choose_image_model');if(f)set('path',f.path)}catch(e){setError(String(e))}};
 const submit=async()=>{setBusy(true);setError('');try{
   const response=await api('media.extensions.install',{...form,dry_run:!plan});
   if(!plan){setPlan(response);return}
   await reload();await refresh();setActive(null);setMessage(L('Установлено','Installed')+' · '+response.name);
 }catch(e){setError(String(e))}finally{setBusy(false)}};
 const toggle=async(r:Obj)=>{if(!model)return;setBusy(true);setError('');try{
   const current=await api('media.get',{id:model.id});const ids:string[]=current.extension_ids||[];
   await api('media.save',{...current,extension_ids:ids.includes(r.id)?ids.filter(id=>id!==r.id):[...ids,r.id]});await refresh();
 }catch(e){setError(String(e))}finally{setBusy(false)}};
 const source=(url:string)=>window.dispatchEvent(new CustomEvent('struve-open-url',{detail:url}));
 const run=async(task:()=>Promise<void>)=>{setBusy(true);setError('');try{await task()}catch(e){setError(String(e))}finally{setBusy(false)}};
 const changeComfy=(key:string,v:any)=>setComfy(old=>({...collection.comfy,...old,[key]:v}));
 const pickComfy=async(key:string)=>{const f=await invoke<Obj|null>(key==='cli'?'choose_executable':'choose_file',key==='cli'?{}:{folder:true});if(f)changeComfy(key,f.path)};
 const installNode=()=>run(async()=>{const v=await api('media.comfy.install',{id:active?.id,dry_run:!plan,expected_version:plan?.version});if(!plan){setPlan(v);return}await reload();setPlan(null);setMessage(L('Пакет установлен. Перезапустите выбранный ComfyUI и проверьте узел.','Package installed. Restart the selected ComfyUI and check the node.'));setActive(null)});
 const nodeInstalled=(id:string)=>collection.comfy_nodes?.some((r:Obj)=>r.id===id&&r.workspace===collection.comfy?.workspace);
 const rows:Obj[]=market.items.filter((c:Obj)=>c.installed_record?.kind!=='lora');
 const adapters:Obj[]=market.items.filter((c:Obj)=>c.installed_record?.kind==='lora').map((c:Obj)=>c.installed_record);
 return <section className="generation-extensions" aria-label={L('Расширения генерации','Generation extensions')}>
 <div className="ge-market-head"><div className="tabs" aria-label={L('Каталог расширений','Extension library')}>{(['installed','available'] as const).map(k=><button key={k} aria-pressed={section===k} className={section===k?'active':''} onClick={()=>{if(k!==section){setMarket(m=>({...m,items:[]}));setSection(k);setOffset(0)}}}>{k==='installed'?L('Установленные','Installed'):L('Доступные','Available')}{market[k]!=null&&<small>{market[k].toLocaleString(en?'en':'ru')}</small>}</button>)}</div>
 <button className="secondary" disabled={syncing} onClick={()=>void sync(true)}><RefreshCw size={14}/>{syncing?L('Обновляется…','Refreshing…'):L('Обновить каталог','Refresh catalog')}</button></div>
 <small className="ge-market-source">{market.updated?('Comfy Registry · '+new Date(market.updated*1000).toLocaleString(en?'en':'ru')+' · '+market.registry_count.toLocaleString(en?'en':'ru')+' '+L('пакетов','packages')):L('Встроенный каталог','Bundled catalog')}</small>
 {syncError&&<p role="status">{L('Показана последняя доступная копия.','Showing the last available catalog.')} {syncError}</p>}
 <div className="ge-controls"><div className="search"><Search size={16}/><input aria-label={L('Найти по задаче или названию','Search by task or name')} placeholder={L('Найти по задаче или названию…','Search by task or name…')} value={query} onChange={e=>{setQuery(e.target.value);setOffset(0)}}/></div>
 <select aria-label={L('Семейство модели','Model family')} value={family} onChange={e=>{setFamily(e.target.value);setOffset(0)}}><option value="">{L('Все семейства','All families')}</option><option value="unspecified">{L('Семейство не указано','Family unspecified')}</option>{Object.entries(families).map(([k,v])=><option key={k} value={k}>{v}</option>)}</select>
 </div>
 {error&&!active&&<p role="alert" className="alert">{error}</p>}{message&&<p role="status">{message}</p>}
 <details className="ge-comfy"><summary>ComfyUI · {L('локальное подключение','local connection')}</summary>
 <div className="ge-defaults">{[['workspace',L('Каталог ComfyUI','ComfyUI workspace')],['cli','comfy.exe']].map(([key,label])=><label key={key} className="field"><span>{label}</span><div className="image-path"><input value={(comfy??collection.comfy)?.[key]||''} onChange={e=>changeComfy(key,e.target.value)}/><button aria-label={label} onClick={()=>void run(()=>pickComfy(key))}><FolderOpen size={14}/></button></div></label>)}
 <label className="field"><span>{L('Порт локального сервера','Local server port')}</span><input type="number" min="1" max="65535" value={(comfy??collection.comfy)?.port??8188} onChange={e=>changeComfy('port',Number(e.target.value))}/></label>
 <div className="ge-controls"><button className="secondary" disabled={busy||!comfy} onClick={()=>void run(async()=>{await api('media.comfy.save',comfy!);await reload();setComfy(null);setMessage(L('Подключение сохранено','Connection saved'))})}>{L('Сохранить','Save')}</button>
 <button className="secondary" disabled={busy||!!comfy} onClick={()=>void run(async()=>{const v=await api('media.comfy.probe');setMessage('ComfyUI '+(v.version||'')+' · '+v.devices.map((d:Obj)=>d.name).join(', '))})}>{L('Проверить связь','Test connection')}</button>
 {data.instances?.some((i:Obj)=>i.id.startsWith('comfy-cpu-')&&['running','starting'].includes(i.status))?<button className="secondary" disabled={busy} onClick={()=>void run(async()=>{const i=data.instances.find((i:Obj)=>i.id.startsWith('comfy-cpu-')&&['running','starting'].includes(i.status));await api('instance.stop',{id:i.id});await refresh()})}>{L('Остановить сервер CPU','Stop CPU server')}</button>:<button className="secondary" disabled={busy||!!comfy} onClick={()=>void run(async()=>{await api('media.comfy.start_cpu');await refresh();setMessage(L('Сервер CPU запускается. Через несколько секунд проверьте связь.','CPU server starting. Test the connection in a few seconds.'))})}>{L('Запустить для нод на CPU','Start CPU node server')}</button>}</div></div></details>
 {section==='installed'&&collection.installed.length>0&&<section className="ge-defaults"><label className="field"><span>{L('Применять при генерации моделью','Use when this model generates')}</span><select value={chosenModel} onChange={e=>setChosenModel(e.target.value)}><option value="">{L('Выберите модель','Select a model')}</option>{models.map(m=><option key={m.id} value={m.id}>{m.name}</option>)}</select></label>
 {model&&compatibleExtensions(collection.installed,model).map(r=><label className="inline-check" key={r.id}><input type="checkbox" disabled={busy} checked={model.extension_ids?.includes(r.id)||false} onChange={()=>void toggle(r)}/>{r.name}{r.strength!=null?' × '+r.strength:''}</label>)}
 {model&&!compatibleExtensions(collection.installed,model).length&&<p className="form-hint">{L('Для этой модели пока нет установленных расширений.','No extensions installed for this model yet.')}</p>}</section>}
 <div className="ge-grid">{adapters.map((r:Obj)=><article key={r.id} className="connection-card ge-card"><small>{models.find(m=>m.id===r.model_id)?.name||r.model_id}</small><h3>{r.name}</h3><p>{r.description}</p><div className="ge-footer"><span>LoRA × {r.strength}</span><button className="secondary" onClick={()=>{setEditing({...r});setError('')}}>{L('Настроить','Configure')}</button></div></article>)}
 {rows.map(c=>{const supported=['local','bundled'].includes(c.backend),done=!!c.installed_record;
 return <article className="connection-card ge-card" key={c.id}><small>{c.families.map((f:string)=>families[f]).join(' · ')||c.publisher||'Comfy Registry'}</small><h3>{c.name}</h3><p className="ge-description">{c[en?'en':'ru']}</p>{c.installed_record?.version&&<small>{L('Установлена версия','Installed version')} {c.installed_record.version}{c.available_version&&c.available_version!==c.installed_record.version?' · '+L('В реестре','In registry')+' '+c.available_version:''}</small>}<div className="ge-footer"><span>{supported?L('Движок Struve','Struve runtime'):c.backend}</span><button className="secondary" onClick={()=>open(c)}>{done?<Check size={14}/>:supported?<Download size={14}/>:<SlidersHorizontal size={14}/>}
 {done?L('Установлено','Installed'):c.id==='lora'?L('Добавить адаптер','Add adapter'):(supported||c.registry)?L('Установить','Install'):L('Как подключить','Setup guide')}</button></div></article>})}</div>
 {market.total>24&&<div className="ge-pages"><button className="secondary" disabled={!market.offset} onClick={()=>setOffset(market.offset-24)}>{L('Назад','Previous')}</button><span>{market.offset+1}–{Math.min(market.offset+24,market.total)} / {market.total}</span><button className="secondary" disabled={market.offset+24>=market.total} onClick={()=>setOffset(market.offset+24)}>{L('Далее','Next')}</button></div>}
 {!rows.length&&!adapters.length&&<p>{L('Расширений по этому запросу нет.','No extensions match this search.')}</p>}
 {editing&&<div className="modal-backdrop"><section className="modal ge-modal" role="dialog" aria-label={editing.name}><div className="modal-heading"><h2>{editing.name}</h2></div><div className="modal-body">{[['name',L('Название','Name')],['description',L('Для чего нужен адаптер','What this adapter does')]].map(([k,label])=><label className="field" key={k}><span>{label}</span><input value={editing[k]} onChange={e=>setEditing({...editing,[k]:e.target.value})}/></label>)}<label className="field"><span>{L('Сила адаптера','Adapter strength')}</span><input type="number" min="0" max="2" step=".05" value={editing.strength} onChange={e=>setEditing({...editing,strength:Number(e.target.value)})}/></label>{error&&<p role="alert">{error}</p>}</div><div className="modal-footer"><button onClick={()=>setEditing(null)} disabled={busy}>{L('Отмена','Cancel')}</button><button className="primary" disabled={busy} onClick={()=>void run(async()=>{await api('media.extensions.configure',editing);await reload();await refresh();setEditing(null)})}>{L('Сохранить','Save')}</button></div></section></div>}
 {active&&<div className="modal-backdrop"><section className="modal ge-modal" role="dialog" aria-modal="true" aria-label={active.name}><div className="modal-heading"><h2>{active.name}</h2><button disabled={busy} aria-label={L('Закрыть','Close')} onClick={()=>setActive(null)}><X size={18}/></button></div>
 <div className="modal-body"><p>{active[en?'en':'ru']}</p><h3>{L('Что понадобится','Requirements')}</h3><p>{active[en?'requires_en':'requires_ru']}</p>
 <button className="text-button" onClick={()=>{setActive(null);source(active.source)}}><ExternalLink size={14}/>{L('Документация автора','Publisher documentation')}</button>
 {active.id==='lora'&&<><label className="field"><span>{L('Базовая модель адаптера','Adapter base model')}</span><select value={form.model_id} onChange={e=>set('model_id',e.target.value)}>{models.map(m=><option key={m.id} value={m.id}>{m.name}</option>)}</select></label>
 <label className="field"><span>{L('Название','Name')}</span><input value={form.name} onChange={e=>set('name',e.target.value)}/></label><label className="field"><span>{L('Для чего нужен адаптер','What this adapter does')}</span><textarea value={form.description} onChange={e=>set('description',e.target.value)}/></label>
 <label className="field"><span>{L('Файл весов','Weights file')}</span><div className="image-path"><input value={form.path} onChange={e=>set('path',e.target.value)}/><button aria-label={L('Выбрать файл','Choose file')} onClick={()=>void choose()}><FolderOpen size={16}/></button></div></label>
 <label className="field"><span>{L('Сила адаптера','Adapter strength')}</span><input type="number" min="0" max="2" step=".05" value={form.strength} onChange={e=>set('strength',Number(e.target.value))}/></label></>}
 {active.backend==='ComfyUI'&&<section className="ge-defaults">{active.registry&&<button className="secondary" disabled={busy} onClick={()=>void run(async()=>setProbe(await api('media.comfy.probe',{id:active.id})))}>{L('Проверить загрузку ноды','Check loaded node')}</button>}
 {probe&&<p role="status">{probe.loaded?L('Нода загружена','Node loaded'):L('Нода ещё не загружена','Node not loaded yet')} · {probe.node}</p>}
 {active.registry&&nodeInstalled(active.id)&&<><button className="secondary" disabled={busy} onClick={()=>void run(async()=>setDeps(await api('media.comfy.dependencies',{id:active.id})))}>{L('Проверить зависимости Python','Check Python dependencies')}</button>
 {deps&&<div role="status">{deps.packages.length?<><p>{L('Будут установлены в среду ComfyUI','Will be installed in the ComfyUI environment')}</p><pre>{deps.packages.join('\n')}</pre><button disabled={busy} className="secondary" onClick={()=>void run(async()=>{await api('media.comfy.dependencies',{id:active.id,plan_id:deps.plan_id});setDeps({...deps,packages:[]})})}>{L('Установить зависимости','Install dependencies')}</button></>:<p>{L('Обязательные библиотеки доступны.','Required libraries are available.')}</p>}{deps.custom_setup&&<p>{L('В пакете также есть сценарий настройки автора. Проверьте его назначение в документации.','This package also includes a publisher setup script. See the documentation for its purpose.')}</p>}</div>}</>}
 {probe?.loaded&&probe.templates?.length>0&&<><label className="field"><span>{L('Готовый граф пакета','Package workflow')}</span><select aria-label={L('Готовый граф пакета','Package workflow')} value={graph} onChange={e=>setGraph(e.target.value)}><option value="">{L('Выберите задачу','Select a task')}</option>{probe.templates.map((t:string)=><option key={t} value={t}>{(active.id==='h3-tools'&&h3Templates[t]?.[en?1:0])||t}</option>)}</select></label>
 <button className="primary" disabled={!graph||busy} onClick={()=>{const url=new URL(probe.url);url.searchParams.set('template',graph);url.searchParams.set('source',probe.template_source);setActive(null);source(url.toString())}}>{L('Открыть выбранный граф','Open selected workflow')}</button></>}
 <button className="secondary" disabled={busy} onClick={()=>void run(async()=>{const v=await api('media.comfy.probe');setActive(null);source(v.url)})}>{L('Открыть графы в правой панели','Open graphs in the right pane')}</button>
 {active.id==='h3-tools'&&probe?.loaded&&<><label className="field"><span>{L('Проверить промпт через ноду','Check a prompt through the node')}</span><textarea value={sample} onChange={e=>setSample(e.target.value)}/></label><button className="secondary" disabled={busy||!sample.trim()} onClick={()=>void run(async()=>{const v=await api('media.comfy.lint',{prompt:sample});setReport(v.report.join('\n'))})}>{L('Запустить проверку','Run check')}</button>{report&&<pre>{report}</pre>}</>}</section>}
 {plan&&<p role="status">{L('Пакет проверен','Package checked')}{plan.bytes?' · '+(plan.bytes/1048576).toFixed(1)+' MiB':''}. {plan.registry_id?plan.registry_id+' '+plan.version+' · '+plan.workspace:L('После установки выберите модель или включите расширение в чате.','After installation, select a model or enable the extension in chat.')}</p>}
 {error&&<p className="alert" role="alert">{error}</p>}</div><div className="modal-footer">
 <button className="secondary" onClick={()=>setActive(null)} disabled={busy}>{L('Закрыть','Close')}</button>
 {active.registry&&!nodeInstalled(active.id)&&<button className="primary" disabled={busy} onClick={()=>void installNode()}>{busy?L('Обработка…','Working…'):plan?L('Установить в ComfyUI','Install in ComfyUI'):L('Проверить установку','Inspect installation')}</button>}
 {['local','bundled'].includes(active.backend)&&!(active.id!=='lora'&&collection.installed.some((r:Obj)=>r.kind===active.id))&&<button className="primary" disabled={busy} onClick={()=>void submit()}>{busy?L('Обработка…','Working…'):plan?L('Установить','Install'):L('Проверить пакет','Check package')}</button>}</div></section></div>}
 </section>
}
