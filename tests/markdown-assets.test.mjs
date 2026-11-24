import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {copyMarkdownAssets} from '../bridge/markdown-assets.mjs';

function fixture(run){
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'struve-md-'));
 try{const source=path.join(root,'source'),output=path.join(root,'output');fs.mkdirSync(source);fs.mkdirSync(output);return run({root,source,output})}
 finally{fs.rmSync(root,{recursive:true,force:true})}
}
test('copies inline and reference images, percent-encoded Unicode paths, and ignores code examples',()=>fixture(({source,output})=>{
 fs.mkdirSync(path.join(source,'images'));fs.writeFileSync(path.join(source,'images','снимок 1.png'),'actual raster bytes');
 fs.writeFileSync(path.join(source,'plot.jpg'),'second image');
 const document=path.join(source,'readme.md');
 const original='# Title\n![One](./images/%D1%81%D0%BD%D0%B8%D0%BC%D0%BE%D0%BA%201.png)\n![Two][plot]\n\n[plot]: plot.jpg\n\n```md\n![fake](missing.png)\n```\n';
 fs.writeFileSync(document,original);const result=copyMarkdownAssets(document,output);
 assert.equal(result.assets.length,2);assert.equal(result.warnings.length,0);
 assert.equal(fs.readFileSync(document,'utf8'),original);
 assert.equal(fs.readFileSync(path.join(output,'images','снимок 1.png'),'utf8'),'actual raster bytes');
 assert.equal(fs.readFileSync(path.join(output,'plot.jpg'),'utf8'),'second image');
}));
test('reports missing, parent, hidden and unsupported images without copying or fetching remote content',()=>fixture(({root,source,output})=>{
 fs.writeFileSync(path.join(root,'outside.png'),'outside');fs.mkdirSync(path.join(source,'.private'));fs.writeFileSync(path.join(source,'.private','p.png'),'hidden');
 const file=path.join(source,'readme.md');fs.writeFileSync(file,'![x](../outside.png)\n![x](.private/p.png)\n![x](missing.png)\n![x](drawing.svg)\n![remote](https://invalid.example/image.png)');
 const result=copyMarkdownAssets(file,output);assert.equal(result.assets.length,0);assert.equal(result.warnings.length,4);assert.deepEqual(fs.readdirSync(output),[]);
}));
test('bounds the copied bytes and image count and preserves earlier successful assets',()=>fixture(({source,output})=>{
 for(const name of ['a','b','c'])fs.writeFileSync(path.join(source,name+'.png'),'123456');
 const file=path.join(source,'readme.md');fs.writeFileSync(file,'![a](a.png)\n![b](b.png)\n![c](c.png)');
 const result=copyMarkdownAssets(file,output,{maxBytes:12,maxFiles:2});assert.equal(result.assets.length,2);assert.equal(result.asset_bytes,12);assert.equal(result.warnings.length,1);assert(!fs.existsSync(path.join(output,'c.png')));
}));
test('rejects a directory junction that points outside the selected document folder',()=>fixture(({root,source,output})=>{
 const outside=path.join(root,'outside');fs.mkdirSync(outside);fs.writeFileSync(path.join(outside,'p.png'),'outside');
 fs.symlinkSync(outside,path.join(source,'alias'),process.platform==='win32'?'junction':'dir');
 const file=path.join(source,'readme.md');fs.writeFileSync(file,'![x](alias/p.png)');
 const result=copyMarkdownAssets(file,output);assert.equal(result.assets.length,0);assert.match(result.warnings[0].reason,/outside/);
}));
