import {sizeCheck} from './documents.mjs';
export async function previewDocx(file){
 await sizeCheck(file);
 const {default:mammoth}=await import('mammoth');
 let imageBytes=0;
 const out=await mammoth.convertToHtml({path:file},{externalFileAccess:false,
  styleMap:["p[style-name='Title'] => h1:fresh","p[style-name='Subtitle'] => p.subtitle:fresh"],
  convertImage:mammoth.images.imgElement(async image=>{
  if(!['image/png','image/jpeg','image/gif','image/webp','image/bmp'].includes(image.contentType))return {alt:'Unsupported embedded image'};
  const buffer=await image.read();imageBytes+=buffer.length;if(imageBytes>4*1024*1024)throw Error('Embedded document images exceed 4 MiB');
  return {src:`data:${image.contentType};base64,${buffer.toString('base64')}`};
 })});
 if(out.value.length>6*1024*1024)throw Error('Document preview exceeds 6 MiB');
 return {html:out.value,warnings:out.messages.map(m=>m.message)};
}
