import test from 'node:test';
import assert from 'node:assert/strict';
import {writeProgress} from '../bridge/progress-file.mjs';
test('A transient Windows sharing violation retries the atomic replacement without rewriting the snapshot',()=>{
 let writes=0,renames=0;const pauses=[];
 writeProgress('progress.json',{content:'Проверено'},{writeFileSync:(file,text)=>{writes++;assert.equal(file,'progress.json.tmp');assert.equal(JSON.parse(text).content,'Проверено')},renameSync:()=>{if(++renames<3)throw Object.assign(Error('sharing violation'),{code:'EPERM'})}},ms=>pauses.push(ms));
 assert.equal(writes,1);assert.equal(renames,3);assert.deepEqual(pauses,[10,20]);
});
test('Persistent lock is bounded, and non-transient storage errors remain visible',()=>{
 let attempts=0;assert.throws(()=>writeProgress('x',{}, {writeFileSync:()=>{},renameSync:()=>{attempts++;throw Object.assign(Error('locked'),{code:'EACCES'})}},()=>{}),/locked/);assert.equal(attempts,9);
 assert.throws(()=>writeProgress('x',{}, {writeFileSync:()=>{throw Object.assign(Error('disk full'),{code:'ENOSPC'})}},()=>assert.fail()),/disk full/);
});
