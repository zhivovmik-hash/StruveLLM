import fs from 'node:fs';

// Windows indexers or a simultaneous reader can briefly deny replacing the file.
// Keep the previous complete JSON until the atomic rename succeeds.
export function writeProgress(filename, value, io=fs, pause=ms=>Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0,ms)) {
  const temporary=filename+'.tmp';
  io.writeFileSync(temporary,JSON.stringify(value));
  for(let attempt=0;;attempt++) {
    try {io.renameSync(temporary,filename);return;}
    catch(error) {
      if(!['EPERM','EACCES','EBUSY'].includes(error.code)||attempt>=8)throw error;
      pause(10*(attempt+1));
    }
  }
}
