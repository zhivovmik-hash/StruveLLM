import {translateError} from './i18n';
import React,{useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
type Obj=Record<string,any>;
type Props={data:Obj,api:(method:string,params?:Obj)=>Promise<any>,ready:(id:string)=>Promise<void>,initial?:boolean};
export default function FirstModel({data,api,ready,initial=false}:Props){
  const en=data.settings.language==='en',L=(ru:string,eng:string)=>en?eng:ru;
  const [mode,setMode]=useState('server'),[protocol,setProtocol]=useState('ollama'),[endpoint,setEndpoint]=useState('http://127.0.0.1:11434');
  const [catalog,setCatalog]=useState<Obj[]|null>(null),[choice,setChoice]=useState(''),[file,setFile]=useState(''),[command,setCommand]=useState(data.paths.llama_server||''),[gpu,setGpu]=useState('');
  const [partial,setPartial]=useState(false),[layers,setLayers]=useState('1'),[vram,setVram]=useState('');
  const [busy,setBusy]=useState(false),[error,setError]=useState(''),[proposal,setProposal]=useState<Obj|null>(null);
  const guarded=async(act:()=>Promise<unknown>)=>{setBusy(true);setError('');try{await act()}catch(e){setError(translateError(String(e)))}finally{setBusy(false)}};
  const scan=()=>guarded(async()=>{
    setCatalog(null);setChoice('');
    const reply=await api('setup.scan',{protocol,endpoint});
    setCatalog(reply.models);setChoice(reply.models[0]?.name||'');
  });
  const pick=(what:'file'|'command')=>guarded(async()=>{
    const chosen=await invoke<Obj|null>(what==='file'?'choose_image_model':'choose_executable',what==='file'?{ggufOnly:true}:{});
    if(chosen){(what==='file'?setFile:setCommand)(chosen.path);setProposal(null)}
  });
  const connect=()=>guarded(async()=>{
    if(mode==='file'&&!proposal){
      const placement=gpu&&partial?{gpu_layers:Number(layers),vram_gib:Number(vram)}:{};
      setProposal(await api('setup.gguf',{file,command,gpu_id:gpu||null,...placement}));return
    }
    const url=endpoint.trim().replace(/\/+$/,'');
    const existing=mode==='server'&&data.models.find((m:Obj)=>m.protocol===protocol&&m.endpoint===url&&m.engine_model===choice);
    const model=existing||await api('model.save',mode==='file'?proposal!:{
      id:'local-'+crypto.randomUUID(),name:choice,engine_model:choice,protocol,endpoint:url,profile_id:'cpu-diagnostics',
      tokenizer:catalog?.find(m=>m.name===choice)?.tokenizer,context_tokens:4096,max_output_tokens:512,temperature:.3,tool_mode:'none',vision:false
    });
    await ready(model.id);
  });
  const usable=(data.hardware.gpus||[]).filter((g:Obj)=>g.compute_backend==='cuda'&&!(data.effective_settings.reserved_gpu_ids||[]).includes(g.id));
  const incomplete=gpu&&partial&&(!Number.isInteger(Number(layers))||Number(layers)<1||Number(layers)>1000||!(Number(vram)>0));
  return <details className="first-model" open={initial||undefined}>
    <summary>{L(initial?'Подключить первую модель':'Подключить локальную модель','Connect a local model')}</summary>
    <section><p>{data.hardware.cpu_name} · RAM {Math.floor(data.hardware.ram_available_gib)} GiB {L('доступно','available')} · {(data.hardware.gpus||[]).map((g:Obj)=>g.name).join(' / ')||'CPU'}</p>
    <small>{data.paths.models} · {Math.floor(data.paths.models_volume?.available_gib||0)} GiB {L('свободно','free')}</small>
    <fieldset className="setup-inputs" disabled={busy}><label>{L('Источник модели','Model source')}<select value={mode} onChange={e=>{setMode(e.target.value);setError('')}}>
      <option value="server">{L('Работающий локальный сервер','Running local server')}</option>
      <option value="file">{L('Файл GGUF на компьютере','Local GGUF file')}</option></select></label>
    {mode==='server'?<>
      <label>{L('Движок','Engine')}<select value={protocol} onChange={e=>{const next=e.target.value;setProtocol(next);setEndpoint(next==='ollama'?'http://127.0.0.1:11434':'http://127.0.0.1:8080/v1');setCatalog(null);setChoice('')}}>
      <option value="ollama">Ollama</option><option value="openai">OpenAI-compatible</option></select></label>
      <label>{L('Локальный адрес','Local endpoint')}<input value={endpoint} onChange={e=>{setEndpoint(e.target.value);setCatalog(null);setChoice('')}}/></label>
      <button className="secondary" disabled={busy} onClick={scan}>{L('Найти модели','Find models')}</button>
      {catalog&&(!catalog.length?<p>{L('Движок доступен. Сначала добавьте в него локальную модель или откройте файл GGUF.','Engine is reachable. Add a model to it or open a local GGUF file.')}</p>:
        <label>{L('Найденная модель','Detected model')}<select value={choice} onChange={e=>setChoice(e.target.value)}>{catalog.map(m=><option key={m.name} value={m.name}>{m.name}{m.bytes?' · '+(m.bytes/1073741824).toFixed(1)+' GiB':''}</option>)}</select></label>)}
    </>:<>
      <label>GGUF<input value={file} placeholder={L('Путь к файлу','File path')} onChange={e=>{setFile(e.target.value);setProposal(null)}}/></label>
      <button className="secondary" onClick={()=>pick('file')}>{L('Выбрать GGUF','Choose GGUF')}</button>
      <label>llama-server.exe<input value={command} onChange={e=>{setCommand(e.target.value);setProposal(null)}}/></label>
      <button className="secondary" onClick={()=>pick('command')}>{L('Выбрать движок llama.cpp','Choose llama.cpp engine')}</button>
      <label>{L('Устройство','Device')}<select value={gpu} onChange={e=>{const card=usable.find((g:Obj)=>g.id===e.target.value);setGpu(e.target.value);setVram(card?String(Math.max(0,Math.floor((card.vram_gib-card.used_gib)*.8*4)/4)):'');setProposal(null)}}><option value="">CPU + RAM</option>{usable.map((g:Obj)=><option key={g.id} value={g.id}>{g.name}</option>)}</select></label>
      {gpu&&<><label>{L('Размещение весов','Weight placement')}<select value={partial?'partial':'full'} onChange={e=>{setPartial(e.target.value==='partial');setProposal(null)}}>
        <option value="full">{L('Все слои на GPU','All layers on GPU')}</option><option value="partial">{L('Часть слоёв на GPU, остальные в RAM','Some layers on GPU, the rest in RAM')}</option></select></label>
        {partial&&<><label>{L('Слоёв на GPU','GPU layers')}<input type="number" min="1" max="1000" value={layers} onChange={e=>{setLayers(e.target.value);setProposal(null)}}/></label>
          <label>{L('Бюджет VRAM, ГиБ','VRAM budget, GiB')}<input type="number" min="0.01" step="any" value={vram} onChange={e=>{setVram(e.target.value);setProposal(null)}}/></label>
          <small>{L('Диспетчер резервирует указанный бюджет. Фактический расход зависит от числа слоёв и контекста.','The scheduler reserves this budget. Actual usage depends on layer count and context size.')}</small></>}</>}
      {proposal&&<p>{proposal.name} · {(proposal.weights_bytes/1073741824).toFixed(2)} GiB · {L('Чтение с диска без копирования · бюджет','Reads in place · budget')} RAM {proposal.resource_profile.ram_gib} GiB · VRAM {proposal.resource_profile.vram_gib} GiB</p>}
    </>}</fieldset>
    {error&&<p role="alert">{error}</p>}
    <button className="primary" disabled={busy||(mode==='server'?!choice:!file||!command||!!incomplete)} onClick={connect}>
      {mode==='file'&&!proposal?L('Проверить размещение','Check placement'):L('Подключить и открыть чат','Connect and open chat')}</button>
    </section>
  </details>
}
export function DiagnosticReport({api,english}:{api:Props['api'],english:boolean}){
 const [report,setReport]=useState(''),[status,setStatus]=useState('');
 const L=(ru:string,en:string)=>english?en:ru;
 return <details className="first-model"><summary>{L('Диагностический отчёт','Diagnostic report')}</summary><section>
   <button className="secondary" onClick={()=>void api('diagnostics.report').then(r=>{setReport(JSON.stringify(r,null,2));setStatus('')}).catch(e=>setStatus(String(e)))}>{L('Показать состав отчёта','Preview report')}</button>
   {report&&<><pre>{report}</pre><button className="secondary" onClick={()=>void invoke<Obj|null>('save_export',{name:'struve-diagnostics.json',content:report}).then(r=>{if(r)setStatus(r.path)}).catch(e=>setStatus(String(e)))}>{L('Сохранить JSON','Save JSON')}</button></>}
   {status&&<p role="status">{status}</p>}</section></details>
}
