import fs from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import {fromMarkdown} from 'mdast-util-from-markdown';

// Preserve the selected Markdown byte-for-byte; copy only its referenced raster images.
export function copyMarkdownAssets(source,destination,{maxFiles=32,maxBytes=20*1024*1024}={}){
 const folder=fs.realpathSync(path.dirname(source)),output=fs.realpathSync(destination);
 const tree=fromMarkdown(fs.readFileSync(source,'utf8')),definitions=new Map(),images=[];
 const pending=[tree];
 while(pending.length){const node=pending.pop();if(node.type==='definition')definitions.set(node.identifier,node.url);
  if(node.type==='image'||node.type==='imageReference')images.push(node);if(node.children)pending.push(...node.children);}
 const assets=[],warnings=[],seen=new Set();let total=0;
 for(const node of images.reverse()){
  const url=node.type==='image'?node.url:definitions.get(node.identifier);if(!url||seen.has(url))continue;seen.add(url);
  // Remote images stay links; importing a document never fetches their URLs.
  if(/^[a-z][a-z\d+.-]*:/i.test(url)||url.startsWith('//'))continue;
  try{
   const relative=decodeURIComponent(url);
   if(path.isAbsolute(relative)||relative.includes(':')||relative.split(/[\\/]/).some(p=>p==='..'||(p!=='.'&&p.startsWith('.'))))
    throw Error('Image must be inside the document folder, outside hidden folders');
   if(!['.png','.jpg','.jpeg','.gif','.webp','.bmp'].includes(path.extname(relative).toLowerCase()))throw Error('Unsupported image format');
   const sourceImage=fs.realpathSync(path.join(folder,relative)),within=path.relative(folder,sourceImage);
   if(path.isAbsolute(within)||within==='..'||within.startsWith('..'+path.sep)||within.split(path.sep).some(p=>p.startsWith('.')))throw Error('Image resolves outside the document folder or into a hidden folder');
   const stat=fs.statSync(sourceImage);if(!stat.isFile())throw Error('Image is not a file');
   if(assets.length>=maxFiles||total+stat.size>maxBytes)throw Error('Markdown image bundle exceeds 32 files or 20 MiB');
   const target=path.join(output,relative);
   fs.mkdirSync(path.dirname(target),{recursive:true});
   // A normalized duplicate path may have a different URL spelling.
   const normalized=path.normalize(relative);
   if(!assets.some(a=>a.path.toLowerCase()===normalized.toLowerCase())){
    fs.copyFileSync(sourceImage,target,fs.constants.COPYFILE_EXCL);total+=stat.size;assets.push({path:normalized,bytes:stat.size});
   }
  }catch(error){if(warnings.length<32)warnings.push({image:url.slice(0,500),reason:error.message.slice(0,500)});}
 }
 return {assets,asset_bytes:total,warnings};
}
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href){
 try{process.stdout.write(JSON.stringify(copyMarkdownAssets(process.argv[2],process.argv[3])))}
 catch(error){process.stderr.write(error.message);process.exitCode=1;}
}
