import {spawn,execFile} from 'node:child_process';
const limit=2*1024*1024;
export async function runCommand(config,args,environment){
 if(!Array.isArray(args)||args.some(a=>typeof a!=='string'||a.includes('\0')))throw Error('Arguments must be strings without NUL');
 return new Promise((resolve,reject)=>{
  const child=spawn(config.command,args,{cwd:config.cwd||undefined,env:{...process.env,...environment},shell:false,windowsHide:true,stdio:['ignore','pipe','pipe']});
  let stdout='',stderr='',bytes=0,done=false;
  const stop=reason=>{if(done)return;done=true;clearTimeout(timer);if(process.platform==='win32'&&child.pid)execFile('taskkill',['/PID',String(child.pid),'/T','/F'],{windowsHide:true},()=>reject(Error(reason)));else{child.kill('SIGKILL');reject(Error(reason))}};
  const timeout=Math.min(180,Math.max(1,config.timeout_seconds||30))*1000;
  const timer=setTimeout(()=>stop(`Command timed out (${timeout/1000} seconds)`),timeout);
  for(const [stream,name] of [[child.stdout,'stdout'],[child.stderr,'stderr']]){stream.setEncoding('utf8');stream.on('data',chunk=>{bytes+=Buffer.byteLength(chunk);if(bytes>limit){stop('Command output exceeds 2 MiB');return}if(name==='stdout')stdout+=chunk;else stderr+=chunk})}
  child.on('error',e=>{if(done)return;done=true;clearTimeout(timer);reject(e)});
  child.on('close',(exit_code)=>{if(done)return;done=true;clearTimeout(timer);resolve({exit_code,stdout,stderr})});
 });
}
export function commandArguments(tool,input={}){
 const schema=tool.inputSchema??{type:'object'},properties=schema.properties??{};
 if(!input||typeof input!=='object'||Array.isArray(input))throw Error('Arguments must be an object');
 for(const key of Object.keys(input)){if(!Object.hasOwn(properties,key))throw Error(`Unknown argument: ${key}`)}
 for(const key of schema.required??[]){if(!Object.hasOwn(input,key))throw Error(`Required argument: ${key}`)}
 for(const [key,value] of Object.entries(input)){const p=properties[key];if(!['string','number','integer','boolean'].includes(p.type))throw Error(`Unsupported argument type: ${key}`);if(p.type==='integer'?!Number.isInteger(value):typeof value!==p.type)throw Error(`Invalid argument type: ${key}`);if(p.enum&&!p.enum.includes(value))throw Error(`Invalid argument value: ${key}`);if(String(value).length>4096||String(value).includes('\0'))throw Error(`Argument too long: ${key}`);if(typeof value==='string'&&(p.pattern&&!new RegExp(p.pattern).test(value)))throw Error(`Argument does not match pattern: ${key}`);if(typeof value==='number'&&(!Number.isFinite(value)||(p.minimum!==undefined&&value<p.minimum)||(p.maximum!==undefined&&value>p.maximum)||(p.exclusiveMinimum!==undefined&&value<=p.exclusiveMinimum)||(p.exclusiveMaximum!==undefined&&value>=p.exclusiveMaximum)))throw Error(`Argument outside numeric bounds: ${key}`);if(typeof value==='string'&&((p.minLength!==undefined&&[...value].length<p.minLength)||(p.maxLength!==undefined&&[...value].length>p.maxLength)))throw Error(`Argument outside length bounds: ${key}`)}
 return (tool.args??[]).map(template=>template.replace(/\{\{([A-Za-z0-9_]+)\}\}/g,(_,name)=>{if(!Object.hasOwn(input,name))throw Error(`Required argument: ${name}`);return String(input[name])}));
}
export async function commandAdapter(config,operation,name,input,environment){
 const tools=(config.tools??[]).map(({name,description,inputSchema})=>({name,description,inputSchema}));
 if(operation==='probe'){let check=null;if(config.probe_args){check=await runCommand(config,[...(config.args??[]),...config.probe_args],environment);if(check.exit_code!==0)throw Error(`Probe failed (${check.exit_code}): ${check.stderr}`)}return {tools,protocol:config.transport==='ide'?'IDE command interface':'CLI',server:{name:config.name},capabilities:{tools:{}},check}}
 if(operation!=='call')throw Error('Unknown bridge operation');
 const tool=config.tools?.find(t=>t.name===name);if(!tool)throw Error('Tool no longer exists on this server');
 const result=await runCommand(config,[...(config.args??[]),...commandArguments(tool,input)],environment);
 if(result.exit_code!==0)throw Error(`Command failed (${result.exit_code}): ${result.stderr||result.stdout}`);
 return {content:[{type:'text',text:result.stdout}],structuredContent:result};
}
