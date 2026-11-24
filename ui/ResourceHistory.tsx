import React,{useEffect,useState} from 'react';
type Obj=Record<string,any>;
type Api=(method:string,params?:Obj)=>Promise<any>;

export function waitExplanation(reason:Obj|undefined,gpus:Obj[],en:boolean){
  if(!reason)return '';
  const choose=(ru:string,english:string)=>en?english:ru;
  const devices=(reason.gpu_ids||[]).map((id:string)=>gpus.find(g=>g.id===id)?.name||id).join(', ');
  const owner=reason.owner_name||reason.owner_id||'';
  switch(reason.code){
    case 'earlier_task':return choose('Впереди задача','Earlier task')+' «'+owner+'» · '+devices;
    case 'media_workflow':return choose('Медиазадача занимает ресурс до завершения','Media task holds the resource until cleanup')+' · '+owner+' · '+devices;
    case 'gpu_process':return choose('Карта занята процессом','GPU is held by a process')+' · '+devices+' · '+owner;
    case 'manual_capacity':return choose('С запущенной вручную моделью','With the manually started model')+' «'+owner+'» · '+reason.needed_gib+' GiB RAM · '+reason.needed_threads+' CPU';
    case 'manual_model':return choose('Модель запущена вручную. Для освобождения карты остановите','Manually started model. To release the GPU, stop')+' «'+owner+'» · '+devices;
    case 'model_in_use':return choose('Модель отвечает в чате','The model is answering in')+' «'+owner+'»'+(devices?' · '+devices:'');
    case 'device_telemetry':return devices+' · '+choose('ожидаем показания свободной VRAM','waiting for available VRAM telemetry');
    case 'device_vram':{
      const occupants=(reason.processes||[]).map((p:Obj)=>(p.name?p.name+' ':'')+'(PID '+p.pid+', '+p.dedicated_mib+' MiB)');
      const board=reason.used_gib!=null?' · '+choose('занято на карте','in use on GPU')+' '+reason.used_gib+' / '+reason.capacity_gib+' GiB':'';
      return devices+' · '+choose('нужно','needs')+' '+reason.needed_gib+' GiB VRAM · '+choose('доступно','available')+' '+reason.available_gib+' GiB'+board+
        (occupants.length?' · '+choose('процессы на карте','processes on GPU')+' '+occupants.join(', '):'');
    }
    case 'system_ram':{
      const units=new Intl.NumberFormat(en?'en-US':'ru-RU',{maximumFractionDigits:2});
      const details=reason.os_available_gib==null?'':' · '+choose('доступно в Windows','available in Windows')+' '+units.format(reason.os_available_gib)+' GiB · '+choose('резерв системы','system reserve')+' '+units.format(reason.system_reserve_gib)+' GiB';
      return choose('Общая RAM с учётом резервов','Shared RAM after reservations')+' · '+choose('нужно','needs')+' '+units.format(reason.needed_gib)+' GiB · '+choose('доступно задаче','available to task')+' '+units.format(reason.available_gib)+' GiB'+details;
    }
    case 'cpu_threads':return choose('Потоки CPU','CPU threads')+' · '+choose('нужно','needs')+' '+reason.needed+' · '+choose('доступно','available')+' '+reason.available;
    default:return reason.code;
  }
}
function gib(bytes:number|null){return bytes==null?'—':(bytes/1073741824).toFixed(2)+' GiB'}
export default function ResourceHistory({data,api,model,context,compact=false}:{data:Obj,api:Api,model?:string,context?:number,compact?:boolean}){
  const en=data.settings.language==='en',L=(ru:string,english:string)=>en?english:ru;
  const [filter,setFilter]=useState(''),[opened,setOpened]=useState(!compact),[rows,setRows]=useState<Obj[]>([]),[error,setError]=useState('');
  const [estimate,setEstimate]=useState<Obj|null>(null);
 const selected=model||filter;
  const stamp=(data.runs||[]).slice(0,5).map((r:Obj)=>r.id+r.status).join(',');
  useEffect(()=>{
    if(!opened)return;
    let alive=true; const refresh=()=>Promise.all([api('resources.history',{model_id:selected||null,limit:compact?10:100}),
      selected?api('resources.estimate',{model_id:selected,context_tokens:context}):Promise.resolve(null)])
      .then(([records,calibration])=>{if(alive){setRows(records);setEstimate(calibration);setError('')}}).catch(e=>{if(alive)setError(String(e))});
    setRows([]);setEstimate(null);void refresh();const timer=setInterval(refresh,10000);return()=>{alive=false;clearInterval(timer)};
  },[selected,opened,stamp,compact,context]);
  const matches=rows.filter(r=>r.configuration_id===data.hardware.configuration_id && (!context||!r.settings.context_tokens||r.settings.context_tokens===context));
  const visible=compact?(matches.length?matches:rows).slice(0,1):rows;
  const models=[...(data.models||[]),...(data.media_models||[])];
  const chosen=models.find(m=>m.id===selected);
  const profile=chosen&&!chosen.runtime&&chosen.protocol?null:estimate?.profile||(data.profiles||[]).find((p:Obj)=>p.id===chosen?.profile_id||p.id==='media-'+chosen?.id);
  return <details className="resource-history" open={opened} onToggle={e=>setOpened(e.currentTarget.open)}>
    <summary>{compact?L('Расход памяти','Memory usage'):L('Фактический расход по моделям','Measured memory by model')}</summary>
    {opened&&<section>{!model&&<select aria-label={L('Модель для замеров','Filter measurements by model')} value={filter} onChange={e=>setFilter(e.target.value)}>
      <option value="">{L('Все модели','All models')}</option>{models.map(m=><option key={m.id} value={m.id}>{m.name}</option>)}</select>}
      {error&&<p role="alert">{error}</p>}
      {profile&&<p>{L('Бюджет профиля','Profile budget')} · RAM {profile.ram_gib} GiB · VRAM {profile.vram_gib} GiB</p>}
      {estimate&&estimate.matching_runs>0&&<>
        <p>{L('Сопоставимых завершённых запусков','Matching completed runs')} · {estimate.matching_runs}.
          {(estimate.suggested_ram_gib!=null||estimate.suggested_vram_gib!=null)&&<>
            {L(' Ориентир с запасом 25% + 0,5 ГиБ',' Suggested budget with 25% + 0.5 GiB headroom')}
            {estimate.suggested_ram_gib!=null&&' · RAM '+estimate.suggested_ram_gib+' GiB'}
            {estimate.suggested_vram_gib!=null&&' · VRAM '+estimate.suggested_vram_gib+' GiB'}
          </>}
        </p>
        {estimate.device_vram_capacity_gib!=null&&estimate.vram_with_headroom_gib>estimate.device_vram_capacity_gib&&
          <p>{L('Пик VRAM','Peak VRAM')} {gib(estimate.measured_dedicated_bytes)}. {L('С полным запасом требуется','Full headroom would require')} {estimate.vram_with_headroom_gib} GiB. {L('Ёмкость карты','GPU capacity')} {estimate.device_vram_capacity_gib} GiB.</p>}
        {estimate.usable_ram_capacity_gib!=null&&estimate.ram_with_headroom_gib>estimate.usable_ram_capacity_gib&&
          <p>{L('Для RAM с полным запасом требуется','RAM with full headroom would require')} {estimate.ram_with_headroom_gib} GiB. {L('После резерва системы доступно до','Capacity after system reserve')} {Number(estimate.usable_ram_capacity_gib.toFixed(2))} GiB.</p>}
      </>}
      {estimate&&estimate.matching_runs===0&&estimate.prior_runs>0&&<small>{L('Для текущих параметров сопоставимых замеров пока нет. Ниже показана история.','No matching measurements for these settings yet. Prior runs are listed below.')}</small>}
      {!visible.length&&!error&&<p>{L('Замеров пока нет. Они появятся после запуска управляемой модели.','No measurements yet. Run a managed model to collect them.')}</p>}
      {visible.map(r=><article key={r.run_id+r.stage}>
        <strong>{r.name||r.model_id}</strong> · {r.stage==='render'?L('генерация','render'):r.stage==='planner'?L('планирование','planner'):L('диалог','chat')}
        {' · '}{r.scope==='external_engine'?L('внешний локальный движок','external local engine'):(r.devices||[]).map((g:Obj)=>g.name).join(' + ')||'CPU'}
        <p>{L('Пиковая RAM процессов','Peak resident RAM')} {gib(r.resident_peak_bytes)} · {L('Выделенная VRAM','Dedicated VRAM')} {gib(r.dedicated_peak_bytes)}</p>
        {!compact&&<small>{L('Пиковая выделенная RAM (commit)','Peak committed RAM')} {gib(r.committed_peak_bytes)}{r.gpu_counter&&' · '+r.gpu_counter}<br/></small>}
        {!compact&&Object.entries(r.dedicated_by_gpu||{}).filter(([,n])=>Number(n)>=1048576).map(([id,n])=><small key={id}>{data.hardware.gpus.find((g:Obj)=>g.id===id)?.name||id} · {gib(Number(n))}<br/></small>)}
        {r.ollama_vram_bytes!=null&&<small>{L('Размещение модели по Ollama','Model placement reported by Ollama')} {gib(r.ollama_vram_bytes)} · </small>}
        <small>{r.settings.context_tokens?L('контекст ','context ')+r.settings.context_tokens:r.settings.width&&r.settings.height?r.settings.width+' × '+r.settings.height:''}
        {r.settings.video_frames?' · '+r.settings.video_frames+L(' кадров',' frames'):''} · {new Date(r.updated_at).toLocaleString(en?'en-US':'ru-RU')} · {r.samples} {L('замеров','samples')}
        {r.run_status&&' · '+(en?r.run_status:({completed:'завершено',running:'выполняется',starting:'запускается',cancelled:'отменено',failed:'ошибка',interrupted:'прервано'} as Obj)[r.run_status]||r.run_status)}{r.configuration_id!==data.hardware.configuration_id&&L(' · другая конфигурация ПК',' · other PC configuration')}
        {context&&r.settings.context_tokens&&context!==r.settings.context_tokens?L(' · другое окно контекста',' · different context'):''}</small>
      </article>)}
    </section>}
  </details>
}
