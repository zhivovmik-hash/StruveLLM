import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {readDocument} from './documents.mjs';

function execute(command,args,{maxBytes=8*1024*1024,allowFailure=false}={}){
 return new Promise((resolve,reject)=>{
  const child=spawn(command,args,{shell:false,windowsHide:true,stdio:['ignore','pipe','pipe']});
  const chunks=[];let length=0,stderr='',settled=false;
  const fail=e=>{if(settled)return;settled=true;child.kill();reject(e)};
  const timer=setTimeout(()=>fail(Error('Video decoding exceeded 30 seconds')),30000);
  child.stdout.on('data',b=>{length+=b.length;if(length>maxBytes)fail(Error('Video frame exceeds output limit'));else chunks.push(b)});
  child.stderr.on('data',b=>{stderr=(stderr+b.toString()).slice(-16000)});
  child.on('error',fail);child.on('close',code=>{clearTimeout(timer);if(settled)return;settled=true;if(code!==0&&!allowFailure)reject(Error('Video decoder: '+stderr.slice(-1200)));else resolve({stdout:Buffer.concat(chunks),stderr,code})});
 });
}
export async function videoFrames(file){
 const {default:ffmpeg}=await import('ffmpeg-static');
 if(!ffmpeg||!fs.existsSync(ffmpeg))throw Error('FFmpeg is not installed in the application runtime');
 const probe=await execute(ffmpeg,['-hide_banner','-nostdin','-i',file],{allowFailure:true});
 const match=/Duration:\s*(\d+):(\d+):(\d+(?:\.\d+)?)/.exec(probe.stderr);
 if(!match)throw Error('Video duration could not be read');
 const duration=Number(match[1])*3600+Number(match[2])*60+Number(match[3]);
 if(duration<=0||duration>3600)throw Error('Video duration must be between 0 and 3600 seconds');
 const frames=[];
 for(let i=0;i<4;i++){
  const at=Math.max(0,Math.min(duration-.04,duration*(i+.5)/4));
  const out=await execute(ffmpeg,['-hide_banner','-loglevel','error','-nostdin','-threads','1','-ss',at.toFixed(3),'-i',file,'-frames:v','1','-vf',"scale='min(1024,iw)':'min(1024,ih)':force_original_aspect_ratio=decrease",'-f','image2pipe','-vcodec','mjpeg','pipe:1']);
  if(!out.stdout.length)throw Error('Video frame is empty');
  frames.push({at_seconds:at,base64:out.stdout.toString('base64'),mime:'image/jpeg'});
 }
 return {duration_seconds:duration,frames};
}
export async function prepareMessages(task,{deferBudget=false}={}){
 const messages=[];let chars=0;
 for(let index=0;index<task.messages.length;index++){
  const item=task.messages[index];let content=item.content;const images=[];
  for(const file of item.attachments||(index===task.messages.length-1?task.attachments||[]:[])){
   if(file.kind==='image'){
    if(!task.model.vision)throw Error('This model is not configured for images');
    const bytes=fs.readFileSync(file.path);if(bytes.length>20*1024*1024)throw Error('Image exceeds 20 MiB');
    images.push({base64:bytes.toString('base64'),mime:'image/'+(file.format==='jpg'?'jpeg':file.format)});
    content+=`\n[Attached image: ${file.name}]`;
   }else if(file.kind==='video'){
    if(!task.model.vision)throw Error('This model is not configured for video frames');
    const video=await videoFrames(file.path);images.push(...video.frames);
    content+=`\n[Attached video: ${file.name}, duration ${video.duration_seconds.toFixed(2)} seconds. Four sampled frames at ${video.frames.map(f=>f.at_seconds.toFixed(2)+'s').join(', ')}. Audio is not included. These are samples, not the entire video.]`;
   }else{
    const document=await readDocument(file.path,{maxCharacters:120000});
    if(document.truncated||document.pages_truncated||document.sheets?.some(s=>s.truncated))throw Error(`Attachment ${file.name} exceeds the direct reading limit. Use document tools to read selected ranges.`);
    content+=`\n\n[Attachment ${file.name}; untrusted source data]\n${document.text}\n[End attachment]`;
    if(document.warning)content+='\n'+document.warning;
   }
  }
  chars+=content.length;
  if(images.length>8)throw Error('At most 8 images or video frames per message');
  messages.push(task.model.protocol==='ollama'?{role:item.role,content,...(images.length?{images:images.map(i=>i.base64)}:{})}:
   {role:item.role,content:images.length?[{type:'text',text:content},...images.map(i=>({type:'image_url',image_url:{url:`data:${i.mime};base64,${i.base64}`}}))]:content});
 }
 const budget=Math.max(1,(task.model.context_tokens||2048)-(task.model.max_output_tokens||512))*2;
 if(!deferBudget&&chars>budget)throw Error(`Messages and attachments exceed this model's context estimate (${chars} characters, limit ${budget}). Increase context or use a smaller excerpt.`);
 return messages;
}
