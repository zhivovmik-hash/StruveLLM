import test from 'node:test';
import assert from 'node:assert/strict';
import {commandAdapter,commandArguments,runCommand} from '../bridge/command.mjs';
const definition={name:'echo',args:['-e','console.log(JSON.stringify(process.argv.slice(1)))','{{text}}'],inputSchema:{type:'object',properties:{text:{type:'string'}},required:['text']}};
test('CLI and IDE execute with literal Unicode arguments and structured results',async()=>{
 for(const transport of ['cli','ide']){const c={transport,command:process.execPath,args:[],probe_args:['--version'],tools:[definition]};const probe=await commandAdapter(c,'probe');assert.equal(probe.tools[0].name,'echo');assert.equal(probe.check.exit_code,0);const special='русский путь "quoted" & echo NEVER $(literal)';const result=await commandAdapter(c,'call','echo',{text:special},{});assert.deepEqual(JSON.parse(result.structuredContent.stdout),[special]);}
});
test('CLI rejects missing, unknown and wrongly typed parameters',()=>{for(const input of [{},{text:4},{text:'x',extra:'y'}])assert.throws(()=>commandArguments(definition,input));});
test('CLI propagates exit failures without a false success',async()=>{await assert.rejects(()=>commandAdapter({command:process.execPath,tools:[{name:'fail',args:['-e','process.exit(3)'],inputSchema:{}}]},'call','fail',{},{}),/Command failed \(3\)/)});
test('CLI enforces declared numeric and Unicode length bounds',()=>{const tool={args:['{{count}}','{{label}}'],inputSchema:{properties:{count:{type:'integer',minimum:1,maximum:4},label:{type:'string',minLength:1,maxLength:3}},required:['count','label']}};assert.deepEqual(commandArguments(tool,{count:2,label:'✓👍'}),['2','✓👍']);for(const input of [{count:0,label:'a'},{count:5,label:'a'},{count:2,label:'four'},{count:2,label:''}])assert.throws(()=>commandArguments(tool,input));});
test('CLI uses its configured timeout for an actual process',async()=>{const start=Date.now();await assert.rejects(runCommand({command:process.execPath,timeout_seconds:1},['-e','setInterval(()=>{},1000)'],{}),/timed out \(1 seconds\)/);assert(Date.now()-start<8000)});
