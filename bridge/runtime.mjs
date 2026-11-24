import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
const spec=JSON.parse(fs.readFileSync(process.argv[2],'utf8'));
const url=new URL(spec.endpoint),runtime=spec.runtime;
const environment={...process.env,...(runtime.environment||{})};
for(const key of ['CUDA_VISIBLE_DEVICES','OMP_NUM_THREADS','MKL_NUM_THREADS','OPENBLAS_NUM_THREADS']){if(process.env[key]!==undefined)environment[key]=process.env[key];else delete environment[key];}
let args=runtime.args||[];
if(runtime.kind==='ollama'){
 const directory=runtime.models_directory||path.join(spec.root,'models','ollama');
 const temp=path.join(spec.root,'runtimes','temporary',spec.id);fs.mkdirSync(directory,{recursive:true});fs.mkdirSync(temp,{recursive:true});
 Object.assign(environment,{OLLAMA_HOST:`${url.hostname}:${url.port||'11434'}`,OLLAMA_MODELS:directory,OLLAMA_NO_CLOUD:'1',OLLAMA_NUM_PARALLEL:'1',OLLAMA_MAX_LOADED_MODELS:'1',OLLAMA_CONTEXT_LENGTH:String(spec.context_tokens||2048),OLLAMA_FLASH_ATTENTION:'0',OLLAMA_VULKAN:'0',GGML_VK_VISIBLE_DEVICES:'-1',OLLAMA_GPU_OVERHEAD:'1073741824',TEMP:temp,TMP:temp});
 args=['serve'];
}else{
 const managedLlama=path.basename(runtime.command).toLowerCase()==='llama-server.exe'
   && args.includes('{{gpu_layers}}');
 if(managedLlama){
   // CUDA renumbers the UUIDs exposed by the Windows Job starting at zero.
   // Requiring these devices makes a CPU-only engine reject a GPU assignment.
   const devices=(environment.CUDA_VISIBLE_DEVICES||'').split(',').filter(id=>id&&id!=='-1');
   if(spec.gpu_layers>0&&!devices.length)throw new Error('No CUDA device assigned for the requested GPU layers');
   const selection=spec.gpu_layers>0?devices.map((_,i)=>'CUDA'+i).join(','):'none';
   if(!args.includes('--device')&&!args.includes('-dev'))args=[...args,'--device',selection];
 }
 args=args.map(arg=>arg.replaceAll('{{context}}',String(spec.context_tokens||2048)).replaceAll('{{gpu_layers}}',String(spec.gpu_layers??0)).replaceAll('{{port}}',url.port||'8080').replaceAll('{{host}}',url.hostname).replaceAll('{{models_dir}}',runtime.models_directory||path.join(spec.root,'models')));
}
const child=spawn(runtime.command,args,{env:environment,cwd:runtime.cwd||path.dirname(runtime.command),shell:false,windowsHide:true,stdio:'inherit'});
child.on('error',error=>{console.error(error.message);process.exitCode=1});
child.on('exit',code=>{process.exitCode=code??1});
