import {translateError} from './i18n';
import {struveLogo} from './branding';
import React,{useEffect,useLayoutEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {ArrowUp,Box,Cpu,Plug,ChevronRight,Paperclip,X,Square,File,Image,Film,RefreshCw,SlidersHorizontal} from 'lucide-react';
import Markdown from './Markdown';
import FirstModel from './FirstModel';
import {SourceExcerpt} from './LibrarySources';
import ResourceHistory,{waitExplanation} from './ResourceHistory';
import ProjectMemory,{type SourceMessage,type Note} from './ProjectMemory';
import type {BrowserRequest} from './BrowserSearch';
import ImageSettings from './ImageSettings';
import {compatibleExtensions} from './GenerationExtensions';
import {webIntent,readIntentURL,previousWebRequest} from '../bridge/web-intent.mjs';
type Obj=Record<string,any>;
export default function Chat({onBrowser,showBrowser,onConfigureModel,data,project,currentChat,draft,setDraft,onChat,nav,api,refresh,perform,busy}:{onBrowser:(r:BrowserRequest|null)=>void,showBrowser:()=>void,onConfigureModel:(id:string)=>void,data:Obj,project:string,currentChat?:Obj,draft:string,setDraft:(s:string)=>void,onChat:(id:string)=>void,nav:(s:string)=>void,api:(method:string,params?:Obj)=>Promise<any>,refresh:()=>Promise<void>,perform:(fn:()=>Promise<any>,message?:string)=>Promise<any>,busy:boolean}){
 const L=(ru:string,en:string)=>data.settings.language==='en'?en:ru;
 const [modelId,setModelId]=useState(''),[toolChoice,setToolChoice]=useState<{project:string,value:boolean|null}|null>(null),[run,setRun]=useState<Obj|null>(null),[files,setFiles]=useState<Obj[]>([]),[uploading,setUploading]=useState(false);
 const explicit=toolChoice?.project===project?(toolChoice.value===null?null:String(toolChoice.value)):localStorage.getItem('chat-tools-enabled-'+project);
 const asksWeb=webIntent(draft,previousWebRequest((currentChat?.messages||[]).filter((m:Obj)=>m.role==='user').map((m:Obj)=>m.content||'')));
 const readURL=readIntentURL(draft);
 const wantsInternet=asksWeb||!!readURL;
 const requiredTool=asksWeb?'web_search':'web_read';
 const useTools=explicit===null?wantsInternet:explicit==='true';
 const setUseTools=(value:boolean)=>{setToolChoice({project,value});localStorage.setItem('chat-tools-enabled-'+project,String(value))};
 const [mode,setMode]=useState('auto'),[mediaId,setMediaId]=useState('');
 const selectedMedia=(data.media_models||[]).find((m:Obj)=>m.id===mediaId);
 const effectiveMedia=selectedMedia||(data.media_models||[]).find((m:Obj)=>m.id===data.media_defaults?.[mode]&&['image','edit','video'].includes(mode));
 const extensions=compatibleExtensions(data.media_extensions||[],effectiveMedia);
 const [override,setOverride]=useState<{key:string,ids:string[]}|null>(null);
 const extensionKey=JSON.stringify([project,currentChat?.id,mode,effectiveMedia?.id]);
 const selectedExtensions=override?.key===extensionKey?override.ids:undefined;
 const checkedExtensions:string[]=selectedExtensions??effectiveMedia?.extension_ids??[];
 useEffect(()=>{setMediaId('')},[currentChat?.id,project,mode]);
 const [toolSelection,setToolSelection]=useState<string[]|null>(null);
 const selectedModel=data.models.find((m:Obj)=>m.id===modelId);
 const maxFiles=selectedModel?.max_attachment_files??8,maxCalls=selectedModel?.max_tool_calls??4;
 const toolsAllowed=maxCalls>0&&selectedModel?.tool_mode!=='none'&&(!Array.isArray(selectedModel?.tool_allowlist)||selectedModel.tool_allowlist.length>0);
 const availableTools=data.connections.filter((c:Obj)=>c.enabled&&c.project_ids.includes(project)).flatMap((c:Obj)=>c.allowed_tools.map((tool:string)=>({key:JSON.stringify([c.id,tool]),connection_id:c.id,connection:c.name,tool}))).filter((t:Obj)=>!Array.isArray(selectedModel?.tool_allowlist)||selectedModel.tool_allowlist.some((a:Obj)=>a.connection_id===t.connection_id&&a.tool===t.tool));
 const selectedTools=availableTools.filter((t:Obj)=>toolSelection===null||toolSelection.includes(t.key));
 const taskTools=explicit===null&&wantsInternet?selectedTools.filter((t:Obj)=>t.connection_id==='internet'&&['web_search','web_read','web_download'].includes(t.tool)&&(asksWeb||t.tool!=='web_search')&&(t.tool!=='web_download'||/скача|сохрани|download|save/i.test(draft))):selectedTools;
 const changeTools=(value:string[]|null)=>{setToolSelection(value);localStorage.setItem('chat-tools-'+project,JSON.stringify(value))};
 useEffect(()=>{try{setToolSelection(JSON.parse(localStorage.getItem('chat-tools-'+project)||'null'))}catch{setToolSelection(null)}},[project]);
 const scroll=useRef<HTMLDivElement>(null),follow=useRef(true),lastScrollTop=useRef(0);
 const scrollToLatest=()=>{
   const view=scroll.current;
   if(view&&follow.current&&!view.querySelector('.welcome')){view.scrollTop=view.scrollHeight;lastScrollTop.current=view.scrollTop}
 };
 const running=run&&['running','starting','queued'].includes(run.status);
 const h3=run?.render_progress;
 const h3References=Boolean(run?.source?.attachment||run?.video_reference_end||run?.video_extension);
 const h3Status=h3?.stage==='conditioning'?(h3References?L('H3 кодирует промпт и референсы…','H3 is encoding the prompt and references…'):L('H3 кодирует промпт…','H3 is encoding the prompt…'))
   :h3?.stage==='diffusion'?L(`H3 · завершено шагов: ${h3.steps_completed} из ${h3.steps_total}`,`H3 · ${h3.steps_completed} of ${h3.steps_total} steps complete`)
   :h3?.stage==='vae_decode'?L('H3 декодирует кадры…','H3 is decoding frames…')
   :h3?.stage==='writing_video'?L('H3 сохраняет видео…','H3 is writing the video…'):null;
 const started=Date.parse(run?.created_at||'');
 const elapsedSeconds=Number.isFinite(started)?Math.max(0,Math.floor((Date.now()-started)/1000)):0;
 const elapsedTime=[Math.floor(elapsedSeconds/3600),Math.floor(elapsedSeconds/60)%60,elapsedSeconds%60].map(n=>String(n).padStart(2,'0')).join(':');
 const currentId=currentChat?.id;
 const [memory,setMemory]=useState<SourceMessage|'list'|null>(null);
 const [showContext,setShowContext]=useState(false),[contextInput,setContextInput]=useState('');
 const [sourceTarget,setSourceTarget]=useState<{chat:string,anchor:string}|null>(null);
 const activeContext=currentChat?.context_tokens??selectedModel?.context_tokens??2048;
 const contextLabel=currentChat?.context_tokens==null?L('Авто','Auto')+' · '+activeContext:String(activeContext);
 useEffect(()=>{setMemory(null);setShowContext(false)},[project,currentId]);
 const [excerpt,setExcerpt]=useState<Obj|null>(null);
 const openSource=(href:string)=>{
   const library=/^#\/library\/(\d+)\/([a-f0-9]{64})\/(\d+)$/.exec(href);
   if(library){void perform(async()=>{const f=await api('library.source',{project_id:project,file_id:Number(library[1]),digest:library[2],anchor:Number(library[3])});setExcerpt(f)},'').catch(()=>{});return}

   const parts=/^#\/source\/([a-zA-Z0-9_-]+)\/([a-zA-Z0-9_-]+)\/(user|assistant)$/.exec(href);
   if(!parts)return;
   const [,chatId,runId,role]=parts;
   const target=data.chats.find((c:Obj)=>c.id===chatId&&c.project_id===project);
   if(!target?.messages?.some((m:Obj)=>m.run_id===runId&&m.role===role)){
     void perform(async()=>{throw Error(L('Исходное сообщение недоступно.','The source message is unavailable.'))},'').catch(()=>{});return;
   }
   setSourceTarget({chat:chatId,anchor:'msg-'+runId+'-'+role});onChat(chatId);
 };
 const applyContext=async(value:string)=>{
   if(!/^(auto|[0-9]+)$/i.test(value))throw Error(L('Введите /context auto или /context 8192.','Enter /context auto or /context 8192.'));
   const saved=await api('chat.context',{project_id:project,chat_id:currentId,model_id:modelId,tokens:value.toLowerCase()==='auto'?null:Number(value)});
   onChat(saved.id);setShowContext(false);
 };
 const browserRequest=running&&run?.phase==='browser_search'&&run?.chat_id===currentId?run?.browser_search:null;
 useEffect(()=>{onBrowser(browserRequest||null);return()=>onBrowser(null)},[browserRequest?.key,onBrowser]);
 const previewFile=(file:Obj)=>{window.dispatchEvent(new CustomEvent('struve-open-file',{detail:{path:file.path}}))};
 const filesKey='draft-files-'+project+'-'+(currentId||'new');
 const updateFiles=(value:Obj[])=>{setFiles(value);localStorage.setItem(filesKey,JSON.stringify(value.map(f=>f.id)))};
 useEffect(()=>{const saved=localStorage.getItem('chat-model-'+project);setModelId(currentChat?.model_id||(data.models.some((m:Obj)=>m.id===saved)?saved:data.models[0]?.id)||'')},[currentId,project,data.models.length]);
 useEffect(()=>{let live=true;setFiles([]);let ids:string[]=[];try{ids=JSON.parse(localStorage.getItem(filesKey)||'[]').slice(0,8)}catch{};void Promise.all(ids.map(id=>api('attachment.get',{id}).catch(()=>null))).then(values=>{if(live)setFiles(values.filter(Boolean))});return()=>{live=false}},[filesKey]);
 useEffect(()=>{let live=true;let timer:ReturnType<typeof setTimeout>;const poll=async()=>{if(!currentChat?.last_run_id){setRun(null);return}try{const next=await api('run.get',{id:currentChat.last_run_id});if(!live)return;setRun(next);if(['running','starting','queued'].includes(next.status))timer=setTimeout(poll,900);else if(!currentChat.messages.some((m:Obj)=>m.run_id===next.id&&m.role==='assistant')&&next.status==='completed')await refresh()}catch(e){if(live)setRun({status:'failed',error:String(e)})}};void poll();return()=>{live=false;clearTimeout(timer)}},[currentChat?.last_run_id,currentChat?.messages.length]);
 useLayoutEffect(()=>{
   follow.current=true;
   const view=scroll.current;
   if(view?.querySelector('.welcome'))view.scrollTop=0;else scrollToLatest();
   if(!view)return;
   const resize=new ResizeObserver(scrollToLatest);
   resize.observe(view);
   if(view.firstElementChild)resize.observe(view.firstElementChild);
   return()=>resize.disconnect();
 },[currentId]);
 useLayoutEffect(scrollToLatest,[run?.content,run?.steps?.length,currentChat?.messages.length]);
 useLayoutEffect(()=>{
   if(sourceTarget?.chat!==currentId||!sourceTarget)return;
   const target=document.getElementById(sourceTarget.anchor);
   if(target){follow.current=false;target.focus({preventScroll:true});target.scrollIntoView({block:'center'});}
 },[sourceTarget,currentId]);
 const send=()=>{void perform(async()=>{
   if(/^\/context(?:\s|$)/i.test(draft.trim())){await applyContext(draft.trim().split(/\s+/).slice(1).join(' '));setDraft('');return}
   if(wantsInternet&&['auto','text'].includes(mode)&&(!useTools||!toolsAllowed||!taskTools.some((t:Obj)=>t.connection_id==='internet'&&t.tool===requiredTool)))throw Error(L('Включите инструменты чата и разрешите '+requiredTool+' в подключении Internet · Web.','Enable chat tools and allow '+requiredTool+' in Internet · Web.'));const response=await api('chat.send',{desktop_browser:true,project_id:project,chat_id:currentId,model_id:modelId,mode:wantsInternet&&mode==='auto'?'text':mode,media_model_id:mediaId||undefined,media_extension_ids:extensions.length?selectedExtensions:undefined,content:draft.trim()||L('Проанализируй приложенные файлы.','Analyze the attached files.'),attachment_ids:files.map(f=>f.id),use_tools:useTools&&toolsAllowed,tool_selection:useTools&&toolsAllowed?taskTools.map(({connection_id,tool}:Obj)=>({connection_id,tool})):[]});onChat(response.chat_id);setDraft('');setOverride(null);updateFiles([]);follow.current=true},'').catch(()=>{})};
 const attach=()=>{setUploading(true);void perform(async()=>{const selected=await invoke<any>('choose_attachments');if(selected?.length){const combined=[...files,...selected].filter((f,i,a)=>a.findIndex(g=>g.id===f.id)===i);if(combined.length>maxFiles)throw Error(L('Лимит вложений этой модели: ','Attachment limit for this model: ')+maxFiles);updateFiles(combined)}},'').catch(()=>{}).finally(()=>setUploading(false))};
 const stop=()=>void perform(()=>api('run.cancel',{id:run?.id}),'').then(next=>setRun(next)).catch(()=>{});
 const steps=(items:Obj[])=>!!items?.length&&<details className="agent-steps"><summary>{L('Вызовы инструментов','Tool calls')} · {items.length}</summary>{items.map((s,i)=><div className="agent-step" key={i}><strong>{s.tool}</strong><small>{s.status}</small><details><summary>{L('Аргументы и результат','Arguments and result')}</summary><pre>{JSON.stringify(s.arguments,null,2)}</pre><pre>{s.result||'…'}</pre></details></div>)}</details>;
 return <div className="chat-page"><div className="chat-scroll" ref={scroll} onScroll={e=>{
   const view=e.currentTarget;
   if(view.scrollHeight-view.scrollTop-view.clientHeight<40)follow.current=true;
   else if(view.scrollTop<lastScrollTop.current-1)follow.current=false;
   lastScrollTop.current=view.scrollTop;
 }}>
 {!currentChat?<div className="welcome"><img className="welcome-logo" src={struveLogo} alt="Struve"/><p className="eyebrow">{L('СТРУВЕ · ЛОКАЛЬНАЯ ЛАБОРАТОРИЯ','STRUVE · LOCAL LAB')}</p><h1>{L('С чего начнём?','What shall we work on?')}</h1><p className="subtitle">{L('Проекты, модели и инструменты','Projects, models and tools')}<br/>{L('в одном рабочем пространстве.','in one workspace.')}</p>{!data.models.length&&<FirstModel data={data} api={api} initial ready={async(id)=>{localStorage.setItem('chat-model-'+project,id);await refresh();setModelId(id)}}/>}<div className="suggestions"><button onClick={()=>nav('resources')}><Cpu/><strong>{L('Распределить ресурсы','Allocate resources')}</strong><span>{L('GPU, память и профили нагрузок','GPU, memory and workload profiles')}</span><ChevronRight/></button><button onClick={()=>nav('extensions')}><Plug/><strong>{L('Подключить инструменты','Connect tools')}</strong><span>{L('Документы, плагины и MCP','Documents, plugins and MCP')}</span><ChevronRight/></button><button onClick={()=>nav('models')}><Box/><strong>{L('Подключить модель','Connect a model')}</strong><span>{L('Ollama и OpenAI-совместимые серверы','Ollama and OpenAI-compatible servers')}</span><ChevronRight/></button></div></div>:<div className="messages">
 {currentChat.messages.map((m:Obj,i:number)=><div id={m.run_id?'msg-'+m.run_id+'-'+m.role:undefined} tabIndex={-1} className={'message '+m.role+(sourceTarget?.chat===currentId&&sourceTarget?.anchor==='msg-'+m.run_id+'-'+m.role?' source-highlight':'')} key={i}><span className="avatar">{m.role==='assistant'?<img src={struveLogo} alt=""/>:L('В','Y')}</span><div><strong>{m.role==='assistant'?(m.kind==='video'?L('Struve · видео','Struve · video'):m.kind==='image'?L('Struve · изображение','Struve · image'):(data.models.find((v:Obj)=>v.id===m.model_id)?.name||'Struve')):L('Вы','You')}{m.kind==='draft'&&<small>{L('Черновик','Draft')}</small>}{m.kind==='cancelled'&&<small>{L('Остановлено · частичный ответ','Stopped · partial answer')}</small>}{m.kind==='interrupted'&&<small>{L('Прервано при перезапуске','Interrupted by restart')}</small>}</strong>{m.attachments?.length>0&&<div className="message-files">{m.attachments.map((f:Obj)=><button key={f.id} type="button" className="attachment-open"  title={L('Открыть в правой панели','Open in preview panel')} onClick={()=>previewFile(f)}><File size={13}/>{f.name}</button>)}</div>}<FileNotes files={m.attachments||[]} english={data.settings.language==='en'}/>{m.role==='assistant' && !m.attachments?.some((f:Obj)=>f.kind==='image') && /(?:^|[.\n]\s*)\[attached image:\s*[^\]\n]+\.(?:png|jpe?g|webp)\]\s*$/i.test(m.content||'') && <p className="inline-note" role="note">{L('В этом ответе нет файла изображения.','This reply has no image attachment.')}</p>}<Markdown content={m.content} onSource={href=>{if(m.memory_sources?.some((n:Note)=>n.source===href))openSource(href)}}/>
 {m.run_id&&m.content&&<button type="button" className="memory-link" onClick={()=>setMemory({chat_id:currentChat.id,run_id:m.run_id,role:m.role,text:m.content})}>{L('Запомнить','Remember')}</button>}
 {!!m.memory_sources?.length&&<details className="agent-steps"><summary>{L('Передано из памяти проекта','Project notes sent to model')} · {m.memory_sources.length}</summary>{m.memory_sources.map((n:Note,i:number)=><button key={n.source} className="memory-link" onClick={()=>openSource(n.source)}>{n.relative||L('Сообщение','Message')} {i+1}</button>)}</details>}{m.kind==='video'&&m.attachments?.filter((f:Obj)=>f.kind==='video').map((f:Obj)=><GeneratedVideo key={f.id} file={f} open={previewFile} english={data.settings.language==='en'}
  savedCopy={m.generation?.exposure_copy?.attachment}
  level={m.generation?.video_reference_end?.attachment && (m.generation?.target_video_frames || m.generation?.config?.video_frames) <= 362
    ? async()=>{const copy=await api('media.level_exposure',{run_id:m.run_id});await refresh();return copy.attachment}
    : undefined}/>)}{['image','video'].includes(m.kind)&&<>{m.attachments?.filter((f:Obj)=>f.kind==='image').map((f:Obj)=><GeneratedImage key={f.id} file={f} open={()=>previewFile(f)} english={data.settings.language==='en'}/>)}{m.kind==='image'&&m.generation?.source?.attachment&&<div className="image-edit-source"><button className="secondary" onClick={()=>previewFile(m.generation.source.attachment)}>{L('Открыть предыдущую версию','Open previous version')}</button><small>{m.generation.edit_plan?.operation==='recolor'?L('Смена цвета · CPU','Recolor · CPU'):L('Правка по маске · GPU','Masked edit · GPU')} · {L('Изменено пикселей: ','Changed pixels: ')}{m.generation.edit_metrics?.changed_pixels} · {L('Вне маски: ','Outside mask: ')}{m.generation.edit_metrics?.outside_mask_changed_pixels}</small></div>}<details className="generation-details"><summary>{L('Промпт и параметры','Prompt and parameters')}{m.generation?.seed!=null&&<> · seed {m.generation.seed}</>}</summary><p>{m.generation?.prompt}</p><p>{m.generation?.width||m.generation?.config?.width} × {m.generation?.height||m.generation?.config?.height} · {m.generation?.config?.steps} {L('шагов','steps')} · CFG {m.generation?.config?.cfg_scale}</p><p>{m.generation?.config?.model_path}</p>{m.kind==='video'&&<p>{m.generation?.target_video_frames||m.generation?.config?.video_frames} {L('кадров','frames')} · {((m.generation?.target_video_frames||m.generation?.config?.video_frames||0)/24).toFixed(2)} {L('с','s')}{m.generation?.video_extension?.attachment&&<> · <button onClick={()=>previewFile(m.generation.video_extension.attachment)}>{L('Исходный ролик','Original clip')}</button></>}</p>}{m.kind==='video'&&m.generation?.source?.attachment&&<button onClick={()=>previewFile(m.generation.source.attachment)}>{L('Первый кадр · референс','First frame · reference')}</button>}{m.kind==='video'&&m.generation?.video_reference_end?.attachment&&<button onClick={()=>previewFile(m.generation.video_reference_end.attachment)}>{L('Последний кадр · референс','Last frame · reference')}</button>}{m.kind==='video'&&typeof m.generation?.config?.reference_exposure_used==='boolean'&&<p>{m.generation.config.reference_exposure_used?L('Освещение привязано к референсам','Lighting guided by keyframes'):L('Освещение по промпту','Lighting from prompt')}</p>}{m.generation?.config?.applied_extensions?.length>0&&<div><p>{L('Расширения','Extensions')} · {m.generation.config.applied_extensions.map((e:Obj)=>e.name+(e.strength!=null?' × '+e.strength:'')).join(', ')}</p>{m.generation.config.prompt_lint_warnings?.length>0&&<ul>{m.generation.config.prompt_lint_warnings.map((w:string,i:number)=><li key={i}>{w}</li>)}</ul>}</div>}<p>GPU: {m.generation?.config?.gpu_id}</p><p>{m.generation?.planner_kept_loaded?L('LLM была доступна чатам во время генерации','LLM was available to chats during rendering'):m.generation?.llm_restored?L('Движок LLM возвращён','LLM runtime restored'):L('Движок LLM остановлен','LLM runtime stopped')}</p>{m.generation?.cleanup_error&&<p role="alert">{m.generation.cleanup_error}</p>}</details></>}{steps(m.steps)}{m.usage?.output_tokens&&<small className="response-metrics">{m.usage.output_tokens} {L('токенов','tokens')}{m.usage.tokens_per_second?` · ${m.usage.tokens_per_second.toFixed(1)} tok/s`:''}{m.finish_reason==='length'?L(' · Достигнут лимит ответа',' · Output limit reached'):''}</small>}</div></div>)}
 {run&&run.status!=='completed'&&!currentChat.messages.some((m:Obj)=>m.role==='assistant'&&m.run_id===run.id)&&<div className="message assistant"><span className="avatar"><img src={struveLogo} alt=""/></span><div><strong>{run.status==='queued'?L('В очереди','Queued'):running?L('Локальная модель работает','Local model is working'):run.status==='failed'?L('Задача завершилась с ошибкой','Task failed'):run.status==='interrupted'?L('Выполнение прервано','Task interrupted'):L('Задача остановлена','Task stopped')}</strong>{run.status==='queued'&&run.wait_reason&&<p role="status">{waitExplanation(run.wait_reason,data.hardware.gpus,data.settings.language==='en')}</p>}{running&&run.stage_wait_reason&&<p role="status">{L('Промпт готов.','Prompt ready.')} {waitExplanation(run.stage_wait_reason,data.hardware.gpus,data.settings.language==='en')}</p>}{run.content&&<Markdown content={run.content}/>}<div className="run-status">{running&&<RefreshCw size={13} className="spin"/>}{(run.error&&translateError(run.error))||(running&&run.stage_wait_reason&&L('Ожидаем ресурсы для следующего этапа','Waiting for resources for the next stage'))||({browser_search:L('Поиск в правой панели…','Searching in the preview panel…'),waiting_for_resources:L('Запуск после освобождения ресурса','Starts when resources become available'),waiting_for_gpu:L('Задача начнётся, когда освободятся модель и GPU.','Starts when the model and GPU are available.'),planning:L('LLM готовит промпт…','LLM is preparing the prompt…'),unloading:L('Освобождение ресурсов LLM…','Releasing LLM resources…'),rendering:run.kind==='video'?(h3Status||L('Генерация и декодирование видео…','Generating and decoding video…')):L('Генерация на GPU…','Generating on GPU…'),encoding:L('Кодирование и проверка видео…','Encoding and validating video…'),video_prepare:L('Подготовка последнего кадра для продолжения…','Preparing the last frame for continuation…'),masking:L('Выделение объекта и локальная правка…','Locating the object and editing locally…'),compositing:L('Сохранение деталей за пределами маски…','Preserving pixels outside the mask…'),cleanup:L('Завершение генерации…','Finishing generation…'),restoring:L('Освобождение GPU и возврат LLM…','Releasing GPU and restoring LLM…'),loading_model:L('Загрузка весов модели','Loading model weights'),attachments:L('Чтение вложений…','Reading attachments…'),starting:L('Подготовка…','Preparing…'),generating:L('Генерация ответа…','Generating…'),tool:L('Вызов инструмента…','Calling tool…'),cancelled:L('Остановлено пользователем','Cancelled'),interrupted:L('Выполнение прервано','Task interrupted'),failed:L('Задача завершилась с ошибкой','Task failed')} as Obj)[running?(run.phase||run.status):run.status]||run.status}</div>{running&&Number.isFinite(started)&&<small className="response-metrics">{L('Прошло: ','Elapsed: ')}{elapsedTime}</small>}{['image','video'].includes(run.kind)&&<><p className="generation-stage">{running&&run.cancel_requested?L('Отмена запрошена; освобождаем ресурсы…','Cancellation requested; releasing resources…'):run.execution?.profile?.gpu_ids?.map((id:string)=>data.hardware.gpus.find((g:Obj)=>g.id===id)?.name||id).join(', ')}</p>{run.prompt&&<details className="generation-details"><summary>{L('Подготовленный промпт','Prepared prompt')}</summary><p>{run.prompt}</p></details>}{run.cleanup_error&&<p role="alert">{run.cleanup_error}</p>}</>}{steps(run.steps)}</div></div>}
 {run&&run.omitted_messages>0&&<div className="context-note">{L('В контекст вошла последняя часть диалога. Более ранних сообщений вне контекста: ','The model received the recent conversation. Earlier messages outside the context: ')}{run.omitted_messages}</div>}
 </div>}
 </div><div className="composer-wrap">{browserRequest&&<button className="secondary" onClick={showBrowser}>{L('Поиск в браузере · открыть вкладку','Browser search · show tab')}</button>}<div className="composer">
 {files.length>0&&<div className="attachment-tray">{files.map(f=><span key={f.id}>{f.kind==='image'?<Image size={14}/>:f.kind==='video'?<Film size={14}/>:<File size={14}/>}<button className="attachment-name"  title={f.name} onClick={()=>previewFile(f)}>{f.name}</button><small>{(f.bytes/1024).toFixed(0)} KB</small><button aria-label={L('Убрать ','Remove ')+f.name} onClick={()=>updateFiles(files.filter(x=>x.id!==f.id))}><X size={12}/></button></span>)}</div>}
 {(mode==='image'||mode==='edit')&&<p className="image-workflow-banner">{L('Новая картинка или правка последней: например, «сделай купол белым». Исходник сохраняется. Можно приложить исходное изображение.','Create or edit the last image: for example, “make the dome white”. The original is preserved. You can attach a source image.')}</p>}{mode==='video'&&<p className="image-workflow-banner">{L('Референсы: первая картинка — начало, вторая — конец ролика. Задайте действия и длительность до 15 секунд.','References: first image is the opening frame; second is the ending frame. Specify actions and a duration up to 15 seconds.')}</p>}<FileNotes files={files} english={data.settings.language==='en'}/>{files.length>maxFiles&&<div className="alert">{L('Уберите лишние вложения. Лимит модели: ','Remove extra attachments. Model limit: ')}{maxFiles}</div>}
 <textarea aria-label={L('Поручение','Task')} placeholder={L('Опишите задачу или приложите файлы…','Describe a task or attach files…')} value={draft} onChange={e=>setDraft(e.target.value)} onKeyDown={e=>{if(e.ctrlKey&&e.key==='Enter'&&(draft.trim()||files.length)&&modelId&&!running&&!busy&&files.length<=maxFiles){e.preventDefault();send()}}}/>
 <div className="composer-toolbar"><div><select className="chat-mode-select" aria-label={L('Режим поручения','Task mode')} value={mode} disabled={!!running} onChange={e=>setMode(e.target.value)}><option value="auto">{L('Авто','Auto')}</option><option value="text">{L('Диалог','Chat')}</option><option value="image">{L('Изображение','Image')}</option><option value="edit">{L('Правка изображения','Edit image')}</option><option value="video">{L('Видео','Video')}</option></select><select className="chat-media-select" aria-label={L('Модель изображений или видео','Image or video model')} value={mediaId} disabled={!!running} onChange={e=>setMediaId(e.target.value)}><option value="">{L('Генератор: автоматически','Generator: automatic')}</option>{(data.media_models||[]).filter((m:Obj)=>m.enabled&&(mode==='video'?m.kind==='video':mode==='image'||mode==='edit'?m.kind==='image':true)).map((m:Obj)=><option key={m.id} value={m.id}>{m.kind==='video'?'🎬 ':'▧ '}{m.name}</option>)}</select>{extensions.length>0&&<details className="ge-chat"><summary aria-label={L('Расширения для этого сообщения','Extensions for this message')}>{L('Расширения','Extensions')} · {checkedExtensions.length}</summary><section>{extensions.map(e=><label key={e.id} title={e.description||e.name}><input type="checkbox" disabled={!!running} checked={checkedExtensions.includes(e.id)} onChange={ev=>setOverride({key:extensionKey,ids:ev.target.checked?[...checkedExtensions,e.id]:checkedExtensions.filter(id=>id!==e.id)})}/>{e.name}</label>)}</section></details>}<ImageSettings data={data} api={api} refresh={refresh} modelId={selectedMedia?.id||data.media_defaults?.[mode==='video'?'video':mode==='edit'?'edit':'image']} compact/><button aria-label={L('Прикрепить файлы','Attach files')} title={L('Код, документы, изображения и видео','Code, documents, images and video')} onClick={attach} disabled={uploading||busy||files.length>=maxFiles}>{uploading?<RefreshCw size={17} className="spin"/>:<Paperclip size={18}/>}</button><select className="chat-model-select" aria-label={L('Модель для диалога','Chat model')} value={modelId} onChange={e=>{setModelId(e.target.value);localStorage.setItem('chat-model-'+project,e.target.value)}} disabled={!!running}><option value="" disabled>{L('Выберите модель','Choose a model')}</option>{data.models.map((m:Obj)=><option key={m.id} value={m.id}>{m.name}</option>)}</select><button aria-label={L('Настройки выбранной модели','Selected model settings')} title={L('Настройки модели','Model settings')} disabled={!modelId} onClick={()=>onConfigureModel(modelId)}><SlidersHorizontal size={16}/></button><label className="tools-toggle" title={L('Лимит вызовов за поручение: ','Tool calls per task: ')+maxCalls}><input type="checkbox" checked={useTools&&toolsAllowed} onChange={e=>setUseTools(e.target.checked)} disabled={!!running||!toolsAllowed}/><Plug size={14}/>{L('Инструменты','Tools')}</label>{explicit!==null&&<button type="button" disabled={!!running} title={L('Включать веб-поиск по тексту поручения','Enable search when the message requests it')} onClick={()=>{localStorage.removeItem('chat-tools-enabled-'+project);setToolChoice({project,value:null})}}>{L('Авто','Auto')}</button>}{useTools&&toolsAllowed&&<details className="chat-tools-menu"><summary>{L('Выбрать','Select')} · {taskTools.length}</summary><div><strong>{L('Инструменты для этого поручения','Tools for this task')}</strong><div className="tool-selection-actions"><button onClick={()=>changeTools(null)}>{L('Все','All')}</button><button onClick={()=>changeTools([])}>{L('Снять выбор','Clear')}</button></div>{availableTools.length?availableTools.map((t:Obj)=><label key={t.key}><input type="checkbox" disabled={!!running} checked={selectedTools.some((s:Obj)=>s.key===t.key)} onChange={e=>changeTools(e.target.checked?[...selectedTools.map((s:Obj)=>s.key),t.key]:selectedTools.map((s:Obj)=>s.key).filter((k:string)=>k!==t.key))}/><span>{t.tool}<small>{t.connection}</small></span></label>):<p>{L('Сначала разрешите инструменты в расширениях.','Enable project tools in Extensions first.')}</p>}<small>{L('Выбирайте нужные инструменты: их описания занимают контекст модели.','Choose relevant tools: their descriptions use model context.')}</small></div></details>}</div>{running?<button className="send stop" aria-label={run?.status==='queued'?L('Снять с очереди','Remove from queue'):L('Остановить ответ','Stop response')} title={run?.status==='queued'?L('Снять с очереди','Remove from queue'):L('Остановить ответ','Stop response')} onClick={stop}><Square size={18}/></button>:<button className="send" disabled={(!draft.trim()&&!files.length)||!modelId||busy||files.length>maxFiles} aria-label={L('Отправить поручение','Send task')} title="Ctrl+Enter" onClick={send}><ArrowUp size={20}/></button>}</div>
 </div><div className="chat-context"><ResourceHistory data={data} api={api} model={effectiveMedia?.id||selectedModel?.id} context={effectiveMedia?undefined:activeContext} compact/><button onClick={()=>setMemory('list')}>{L('Память проекта','Project memory')}</button>
 <button disabled={!modelId} aria-expanded={showContext} onClick={()=>{setContextInput(String(activeContext));setShowContext(!showContext)}}>{L('Контекст','Context')} · {contextLabel}</button>
 {showContext&&<form onSubmit={e=>{e.preventDefault();void perform(()=>applyContext(contextInput),'').catch(()=>{})}}>
 <input type="number" min={Math.max(512,(selectedModel?.max_output_tokens??512)+1)} max={131072} aria-label={L('Токены контекста','Context tokens')} value={contextInput} onChange={e=>setContextInput(e.target.value)}/>
 <button disabled={busy}>{L('Применить','Apply')}</button><button type="button" disabled={busy} onClick={()=>void perform(()=>applyContext('auto'),'').catch(()=>{})}>{L('Авто','Auto')}</button>
 <small>{L('Со следующего сообщения · резерв ответа ','Next message · output reserve ')}{selectedModel?.max_output_tokens??512}{run?.usage?.input_tokens?' · '+L('последний ввод ','last input ')+run.usage.input_tokens:''}
 {selectedModel?.protocol==='ollama'?L(' · возможна перезагрузка модели',' · model may reload'):L(' · действует в пределах окна сервера',' · limited by the server window')}</small>
 {run?.context_usage&&<small className="context-use">{run.context_usage.counted_by&&<strong>{L('Токенизатор движка','Engine tokenizer')} · {run.context_usage.prompt_tokens} · </strong>}{L('Оценка первого запроса без визуальных токенов','First request estimate, excluding visual tokens')} · {Object.entries(run.context_usage.estimated_tokens).map(([key,n])=>({instructions:L('инструкции','instructions'),tools:L('инструменты','tools'),memory:L('память','memory'),history:L('история','history'),message:L('сообщение','message'),attachments:L('вложения','attachments')} as Record<string,string>)[key]+' '+n).join(' · ')}</small>}</form>}</div>
 {excerpt&&<SourceExcerpt source={excerpt} en={data.settings.language==='en'} close={()=>setExcerpt(null)}/>}
 {memory&&<ProjectMemory project={project} english={data.settings.language==='en'} source={memory==='list'?undefined:memory} api={api} openSource={openSource} close={()=>setMemory(null)}/>}
 <p className="composer-hint">{!selectedModel?L('Подключите локальную модель в разделе «Модели».','Connect a local model in Models.'):selectedModel.vision?L('Зрение включено · видео передаётся как выборка кадров','Vision enabled · video is sent as sampled frames'):L('Текстовая модель · код и документы','Text model · code and documents')}{selectedModel&&<span className="chat-model-summary"> · T {selectedModel.temperature??0.3} · {L('навыков','skills')}: {(selectedModel.skills||[]).filter((s:Obj)=>s.enabled).length}</span>}</p></div></div>;
}

function GeneratedImage({file,open,english}:{file:Obj,open:()=>void,english:boolean}){
 const [url,setUrl]=useState(''),[error,setError]=useState('');
 useEffect(()=>{let live=true;void invoke<Obj>('preview_path',{path:file.path}).then(v=>{if(live)setUrl(v.url)}).catch(e=>{if(live)setError(String(e))});return()=>{live=false}},[file.path]);
 return <figure className="generated-image">{url?<button onClick={open} aria-label={english?'Open generated image':'Открыть созданное изображение'}><img src={url} alt={english?'Locally generated image':'Локально созданное изображение'}/></button>:<p>{error||(english?'Loading image…':'Загрузка изображения…')}</p>}<figcaption>{file.name} · {english?'Click to open in the right panel':'Нажмите для просмотра в правой панели'}</figcaption></figure>;
}

function FileNotes({files,english}:{files:Obj[],english:boolean}){
 const withNotes=files.filter(f=>f.assets?.length||f.warnings?.length);
 return withNotes.length?<div className="attachment-notes">{withNotes.map(f=><details key={f.id}><summary>{f.name} · {english?'Images':'Изображений'}: {f.assets?.length||0}{f.warnings?.length?` · ${english?'check missing images':'есть недоступные картинки'}`:''}</summary><p>{english?'Markdown images are preserved for preview. Attach them separately if the model needs to analyze them.':'Картинки Markdown сохранены для просмотра. Для анализа моделью прикрепите их отдельными файлами.'}</p>{f.warnings?.map((w:Obj,i:number)=><p key={i}>{w.image}: {w.reason}</p>)}</details>)}</div>:null;
}

function GeneratedVideo({file,open,english,savedCopy,level}:{
 file:Obj,open:(file:Obj)=>void,english:boolean,savedCopy?:Obj,level?:()=>Promise<Obj>
}){
 const [useCopy,setUseCopy]=useState(false),[newCopy,setNewCopy]=useState<Obj>();
 const [working,setWorking]=useState(false),[actionError,setActionError]=useState('');
 const [preview,setPreview]=useState<{path:string,url?:string,error?:string}>();
 const copy=savedCopy||newCopy, shown=useCopy&&copy?copy:file;
 useEffect(()=>{
   let live=true;
   void invoke<Obj>('preview_path',{path:shown.path})
     .then(v=>{if(live)setPreview({path:shown.path,url:v.url})})
     .catch(e=>{if(live)setPreview({path:shown.path,error:String(e)})});
   return()=>{live=false};
 },[shown.path]);
 const levelExposure=async()=>{
   if(!level||working)return;
   setWorking(true);setActionError('');
   try{setNewCopy(await level());setUseCopy(true)}
   catch(e){setActionError(String(e))}
   finally{setWorking(false)}
 };
 const live=preview?.path===shown.path?preview:undefined;
 return <figure className="generated-image">
   {live?.url?<video key={shown.path} src={live.url} controls preload="metadata"/>:<p>{live?.error||'…'}</p>}
   <figcaption><button onClick={()=>open(shown)}>{shown.name} ↗</button></figcaption>
   {copy?<div className="video-versions">
     <button type="button" aria-pressed={!useCopy} onClick={()=>setUseCopy(false)}>{english?'Original':'Оригинал'}</button>
     <button type="button" aria-pressed={useCopy} onClick={()=>setUseCopy(true)}>{english?'Exposure matched':'С коррекцией яркости'}</button>
   </div>:level&&<div className="video-versions"><button type="button" disabled={working} onClick={()=>void levelExposure()}
      title={english?'Save a separate copy for a fixed camera, using the lighting of the first and last frames':'Сохранить отдельную копию для неподвижной камеры по освещению первого и последнего кадров'}>
     {working?(english?'Matching exposure…':'Выравниваем яркость…'):(english?'Match exposure':'Выровнять яркость')}
   </button></div>}
   {actionError&&<p role="alert">{actionError}</p>}
 </figure>;
}
