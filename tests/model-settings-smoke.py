"""Real controller -> Windows Job -> Node -> streaming model/MCP -> durable chat."""
import json,os,pathlib,subprocess,tempfile,time,threading,urllib.request,socket,sys
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler
source=pathlib.Path(__file__).resolve().parents[1]
from smoke_paths import test_cli, test_root
root=test_root('model-settings-')
binary=test_cli()
node=root/'app/node.exe'
checks=[];requests=[]
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_POST(self):
  data=json.loads(self.rfile.read(int(self.headers['Content-Length'])));requests.append(data)
  if data['model']=='failure':self.send_response(503);self.end_headers();self.wfile.write(b'fixture unavailable');return
  self.send_response(200);self.send_header('Content-Type','application/x-ndjson');self.end_headers()
  def send(value):self.wfile.write(json.dumps(value,ensure_ascii=False).encode()+b'\n');self.wfile.flush()
  try:
   if data['model']=='slow':
    send({'message':{'content':'Partial answer'}});time.sleep(8);send({'done':True});return
   if data['model'] in ['tool','unknown-tool'] and data['messages'][-1]['role']!='tool':
    send({'message':{'tool_calls':[{'function':{'name':'tool_0' if data['model']=='tool' else 'tool_99','arguments':{}}}]}});send({'done':True});return
   answer='MCP result received' if data['messages'][-1]['role']=='tool' else 'Ответ из локального потока ✓'
   for word in answer.split(' '):send({'message':{'content':word+' '}});time.sleep(.02)
   send({'done':True,'done_reason':'stop','eval_count':8,'eval_duration':1_000_000_000})
  except (ConnectionAbortedError,ConnectionResetError,BrokenPipeError):pass
server=ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
env=dict(os.environ,STRUVE_LLM_ROOT=str(root),STRUVE_LLM_SOURCE=str(source));env.pop('STRUVE_LLM_PORT',None)
process=None
def start():
 global process,endpoint,token
 process=subprocess.Popen([str(binary),'serve'],env=env,stdout=subprocess.DEVNULL,stderr=(root/'stderr.log').open('a'),creationflags=subprocess.CREATE_NO_WINDOW)
 for _ in range(100):
  time.sleep(.1)
  try:
   record=json.loads((root/'data/controller.endpoint.json').read_text());endpoint=f"http://127.0.0.1:{record['port']}/rpc";token=(root/'data/controller.token').read_text()
   if rpc('ping',{})['pid']==process.pid:return
  except Exception:pass
 raise RuntimeError((root/'stderr.log').read_text())
def rpc(method,params,ok=True):
 request=urllib.request.Request(endpoint,data=json.dumps({'method':method,'params':params}).encode(),headers={'Content-Type':'application/json','Authorization':'Bearer '+token})
 value=json.load(urllib.request.urlopen(request,timeout=40));assert value['ok']==ok,value
 return value.get('data',value.get('error'))
def wait(task):
 for _ in range(150):
  value=rpc('run.get',{'id':task['id']})
  if value['status'] not in ['starting','running']:return value
  time.sleep(.1)
 raise AssertionError(value)
def model(name):
 return rpc('model.save',dict(id=name,name=name,protocol='ollama',endpoint=f'http://127.0.0.1:{server.server_port}',engine_model=name,profile_id='cpu-diagnostics',context_tokens=4096,max_output_tokens=128,temperature=0,vision=False))
def send(name='plain',**extra):return rpc('chat.send',dict(project_id='struve',model_id=name,content='ÐŸÑ€Ð¾Ð²ÐµÑ€ÑŒ Ð»Ð¾ÐºÐ°Ð»ÑŒÐ½Ð¾Ðµ Ð¿Ð¾Ð´ÐºÐ»ÑŽÑ‡ÐµÐ½Ð¸Ðµ',**extra))
try:
 start()
 m=model('plain');m.update(top_p=.82,top_k=23,min_p=.04,repeat_penalty=1.13,seed=0,stop=['END'],keep_alive_seconds=0,thinking=False,system_prompt='CUSTOM_RULE_41',skills=[dict(id='enabled',name='Enabled',content='ENABLED_RULE_42',enabled=True),dict(id='disabled',name='Disabled',content='DISABLED_RULE_43',enabled=False)],max_tool_calls=2,agent_timeout_seconds=30,max_attachment_files=1)
 m=rpc('model.save',m)
 for key,value in [('top_p',1.4),('seed',.5),('max_tool_calls',9),('agent_timeout_seconds',0),('max_attachment_files',9)]:
  bad=dict(m);bad[key]=value;rpc('model.save',bad,ok=False)
 assert rpc('model.get',{'id':'plain'})==m
 checks.append('Invalid sampling and policy settings reject without overwriting the saved model')
 task=send();result=wait(task);assert result['status']=='completed',result
 sent=requests[-1];assert sent['options']['top_p']==.82 and sent['options']['seed']==0 and sent['keep_alive']=='0s'
 system=sent['messages'][0]['content'];assert all(t in system for t in ['CUSTOM_RULE_41','ENABLED_RULE_42']) and 'DISABLED_RULE_43' not in system
 assert result['execution']['model']['revision']==m['revision'] and result['execution']['limits']['max_tool_calls']==2
 checks.append('Real controller and worker send configured options, enabled skill and system instructions; run records their revision and profile snapshot')
 rpc('chat.send',dict(project_id='struve',model_id='plain',content='x',max_tool_calls=3),ok=False)
 rpc('chat.send',dict(project_id='struve',model_id='plain',content='x',attachment_ids=['a','b']),ok=False)
 checks.append('Task cannot raise model tool or attachment limits before launch')
 f=root/'SKILL.md';f.write_text('---\nname: imported-skill\n---\nUse the imported evidence rule.',encoding='utf-8')
 cli=subprocess.run([str(binary),'--json','model','skill-import','plain','--file',str(f),'--disabled'],env=env,capture_output=True,encoding='utf-8');assert cli.returncode==0,cli.stderr+cli.stdout
 imported=json.loads(cli.stdout)['data']['skills'][-1];assert imported['name']=='imported-skill' and imported['enabled']==False and 'imported evidence' in imported['content']
 f.write_text('Changed outside after import',encoding='utf-8');assert rpc('model.get',{'id':'plain'})['skills'][-1]['content']==imported['content']
 checks.append('Named CLI imports UTF-8 SKILL.md as a durable instruction snapshot and can leave it disabled')
 config=rpc('mcp.get',{'id':'local-diagnostics'});config.update(enabled=True,project_ids=['struve'],allowed_tools=['runtime_info']);rpc('mcp.save',config);rpc('mcp.probe',{'id':'local-diagnostics'})
 m=model('tool');m.update(max_tool_calls=1,tool_allowlist=[dict(connection_id='local-diagnostics',tool='runtime_info')]);m=rpc('model.save',m)
 rpc('chat.send',dict(project_id='struve',model_id='tool',content='x',use_tools=True,tool_selection=[dict(connection_id='documents',tool='document_read')]),ok=False)
 task=send('tool',use_tools=True);result=wait(task);assert result['status']=='completed' and len(result['steps'])==1 and result['tool_calls_used']==1,result
 checks.append('Model allowlist is inherited and enforces a real MCP call; disallowed request selection is rejected')
 slow=model('slow');slow.update(max_tool_calls=1,agent_timeout_seconds=30,tool_allowlist=[dict(connection_id='local-diagnostics',tool='runtime_info')]);rpc('model.save',slow)
 task=send('slow',use_tools=True)
 rpc('mcp.call',dict(id='local-diagnostics',tool='runtime_info',project_id='struve',run_id=task['id'],arguments={}))
 denied=rpc('mcp.call',dict(id='local-diagnostics',tool='runtime_info',project_id='struve',run_id=task['id'],arguments={}),ok=False);assert 'limit' in denied['message'].lower(),denied
 rpc('run.cancel',{'id':task['id']})
 checks.append('Dispatcher independently counts and blocks tool calls past the saved task limit')
 slow=rpc('model.get',{'id':'slow'});slow['agent_timeout_seconds']=2;rpc('model.save',slow)
 before=time.monotonic();task=send('slow');result=wait(task);elapsed=time.monotonic()-before
 assert result['status']=='failed' and '2 seconds' in result['error'] and elapsed<7,(result,elapsed)
 before_restart=rpc('run.get',{'id':task['id']});process.terminate();process.wait(10);start();assert rpc('run.get',{'id':task['id']})['status']=='failed'
 checks.append('Two-second task deadline aborts actual streaming and persists failure across controller restart')
 m=rpc('model.get',{'id':'plain'});m['resource_profile']=dict(gpu_ids=[],ram_gib=.5,cpu_threads=1,vram_gib=0,exclusive=False)
 before_profiles=len(rpc('profile.list',{}));m=rpc('model.save',m);profile=rpc('profile.get',{'id':m['profile_id']})
 assert m['resource_mode']=='dedicated' and profile['owner_model_id']=='plain' and len(rpc('profile.list',{}))==before_profiles+1
 failed=dict(m);failed['revision']=0;failed['resource_profile']=dict(profile,ram_gib=2)
 rpc('model.save',failed,ok=False);assert rpc('profile.get',{'id':profile['id']})==profile
 checks.append('Individual hardware profile and model save atomically; stale model revision rolls back profile changes')
 draft=dict(m,resource_profile=dict(profile,gpu_ids=['missing-gpu-uuid'],vram_gib=1));plan=rpc('model.plan',draft);assert plan['admissible']==False and plan['reasons']
 assert rpc('model.get',{'id':'plain'})==m
 checks.append('Read-only model plan detects absent GPU against current hardware and leaves settings untouched')
 probe=socket.socket();probe.bind(('127.0.0.1',0));port=probe.getsockname()[1];probe.close()
 managed=dict(id='managed-settings',name='Managed settings',protocol='ollama',endpoint=f'http://127.0.0.1:{port}',engine_model='plain',profile_id='cpu-diagnostics',context_tokens=2048,max_output_tokens=128,temperature=0,runtime=dict(kind='command',command=sys.executable,args=[str(source/'tests/runtime-fixture.py'),'{{port}}']),resource_profile=dict(gpu_ids=[],ram_gib=.5,cpu_threads=1,vram_gib=0,exclusive=False))
 managed=rpc('model.save',managed);instance=rpc('model.start',{'id':managed['id']});assert instance['profile']['owner_model_id']==managed['id'] and instance['profile']['ram_gib']==.5 and instance['profile']['cpu_threads']==1,instance
 current=rpc('model.get',{'id':managed['id']});current['resource_profile']=dict(rpc('profile.get',{'id':current['profile_id']}),ram_gib=1)
 denied=rpc('model.save',current,ok=False);assert 'Stop' in denied['message'],denied
 rpc('model.stop',{'id':managed['id']});rpc('model.save',current)
 checks.append('Managed runtime receives individual CPU/RAM profile; changing live allocation is blocked until explicit stop')
 output=dict(ok=True,version=rpc('doctor',{})['version'],checks=checks,root=str(root),deadline_seconds_observed=round(elapsed,3))
 (root/'model-settings-result.json').write_text(json.dumps(output,ensure_ascii=False,indent=2),encoding='utf-8');print(json.dumps(output,ensure_ascii=False))
finally:
 if process and process.poll() is None:process.terminate();process.wait(10)
 server.shutdown();server.server_close()
