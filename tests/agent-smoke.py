"""Real controller -> Windows Job -> Node -> streaming model/MCP -> durable chat."""
import json,os,pathlib,shutil,subprocess,tempfile,time,threading,urllib.request,socket,sys
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler
source=pathlib.Path(__file__).resolve().parents[1]
from smoke_paths import test_cli, test_root
root=test_root('Struve smoke ')
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
  if value['status'] not in ['queued','starting','running']:return value
  time.sleep(.1)
 raise AssertionError(value)
def model(name):
 return rpc('model.save',dict(id=name,name=name,protocol='ollama',endpoint=f'http://127.0.0.1:{server.server_port}',engine_model=name,profile_id='cpu-diagnostics',context_tokens=4096,max_output_tokens=128,temperature=0,vision=False))
def send(name='plain',**extra):return rpc('chat.send',dict(project_id='struve',model_id=name,content='Проверь локальное подключение',**extra))
try:
 start()
 for name in ['plain','slow','failure','tool','unknown-tool']:model(name)
 task=send();result=wait(task);assert result['status']=='completed',result
 chat=rpc('chat.get',{'id':task['chat_id']});assert chat['messages'][-1]['content'].strip()=='Ответ из локального потока ✓';assert chat['messages'][-1]['usage']['output_tokens']==8
 (root/'jobs'/f"{task['id']}-progress.json").write_text(json.dumps(dict(id=task['id'],status='running',content='stale partial')),encoding='utf-8')
 assert rpc('run.get',{'id':task['id']})['content'].strip()=='Ответ из локального потока ✓'
 checks.append('Streaming answer reaches persistent chat through a bounded Windows Job worker')
 imported_file=root/'sample.md';imported_file.write_text('# Пример\nТекст из вложения\n![Test](sample.png)\n![Missing](absent.png)',encoding='utf-8')
 (root/'sample.png').write_bytes(b'attachment image fixture')
 attachment=rpc('attachment.import',{'path':str(imported_file)})
 assert len(attachment['assets'])==1 and len(attachment['warnings'])==1,attachment
 assert (pathlib.Path(attachment['path']).parent/'sample.png').read_bytes()==(root/'sample.png').read_bytes()
 assert pathlib.Path(attachment['path']).read_bytes()==imported_file.read_bytes()
 task=send(attachment_ids=[attachment['id']]);result=wait(task);assert result['status']=='completed',result
 assert any('Текст из вложения' in m['content'] for m in requests[-1]['messages']);assert rpc('chat.get',{'id':task['chat_id']})['messages'][0]['attachments'][0]['name']=='sample.md'
 checks.append('Attachment import preserves UTF-8 Markdown and referenced image bytes, reports missing assets, and delivers text to the model')
 task=send('tool',use_tools=True,tool_selection=[dict(connection_id='local-diagnostics',tool='runtime_info')]);result=wait(task);assert result['status']=='completed',result
 assert result['steps'][0]['tool']=='runtime_info';assert result['steps'][0]['status']=='completed';assert 'MCP result' in result['content']
 checks.append('Actual MCP discovery, permitted tool dispatch, receipt and model follow-up')
 selected=[dict(connection_id='local-diagnostics',tool='runtime_info')]
 result=wait(send('tool',use_tools=True,tool_selection=selected));assert result['status']=='completed' and result['steps'][0]['tool']=='runtime_info',result
 result=wait(send(use_tools=True,tool_selection=[]));assert result['status']=='completed' and not requests[-1].get('tools'),result
 before_requests=len(requests)
 result=wait(send(use_tools=True,tool_selection=[dict(connection_id='local-diagnostics',tool='not-permitted')]))
 assert result['status']=='failed' and 'no longer enabled' in result['error'] and len(requests)==before_requests,result
 checks.append('Per-task selection exposes only chosen tools; empty selection disables tools and stale selections fail before inference')
 before=len([e for e in rpc('events.list',{}) if e['action']=='mcp.call.started'])
 result=wait(send('unknown-tool',use_tools=True));assert result['status']=='failed',result
 assert len([e for e in rpc('events.list',{}) if e['action']=='mcp.call.started'])==before
 checks.append('Model-invented tools are rejected without execution')
 result=wait(send('failure'));assert result['status']=='failed' and '503' in result['error'],result
 (root/'jobs'/f"{result['id']}-progress.json").unlink(missing_ok=True)
 assert rpc('run.get',{'id':result['id']})['status']=='failed'
 waited=subprocess.run([str(binary),'--json','run','wait',result['id']],env=env,capture_output=True,text=True,encoding='utf-8',timeout=20,creationflags=subprocess.CREATE_NO_WINDOW)
 failed=json.loads(waited.stdout);assert waited.returncode!=0 and not failed['ok'] and failed['error']['code']=='TASK_FAILED' and failed['data']['status']=='failed',failed
 checks.append('Runtime HTTP failure is retained as a failed task, never a fake answer')
 task=send('slow')
 for _ in range(50):
  state=rpc('run.get',{'id':task['id']})
  if state['content']:break
  time.sleep(.1)
 assert state['content']=='Partial answer',state
 rpc('chat.send',{'project_id':'struve','model_id':'plain','chat_id':task['chat_id'],'content':'duplicate'},ok=False)
 cancelled=rpc('run.cancel',{'id':task['id']});assert cancelled['status']=='cancelled' and cancelled['phase']=='done' and cancelled['completed_at']
 assert rpc('instance.get',{'id':task['id']})['status']=='stopped'
 partial=rpc('chat.get',{'id':task['chat_id']})['messages'][-1]
 assert partial['kind']=='cancelled' and partial['content']=='Partial answer',partial
 rpc('run.cancel',{'id':task['id']})
 continued=wait(send('plain',chat_id=task['chat_id']));assert continued['status']=='completed'
 messages=rpc('chat.get',{'id':task['chat_id']})['messages']
 assert len([m for m in messages if m.get('kind')=='cancelled'])==1
 assert not any(m['content']=='Partial answer' for m in requests[-1]['messages'])
 checks.append('Cancel terminates only its worker, records partial output once in chat, and excludes that unfinished answer from later inference; concurrent send is rejected')
 task=send('slow');rpc('instance.stop',{'id':task['id']})
 assert rpc('run.get',{'id':task['id']})['status']=='cancelled'
 checks.append('Generic process stop routes an active chat worker through task cancellation')
 probe=socket.socket();probe.bind(('127.0.0.1',0));port=probe.getsockname()[1];probe.close()
 runtime_model=rpc('model.save',dict(id='managed',name='Managed fixture',protocol='ollama',endpoint=f'http://127.0.0.1:{port}',engine_model='plain',profile_id='cpu-diagnostics',context_tokens=2048,max_output_tokens=128,temperature=0,vision=False,runtime=dict(kind='command',command=sys.executable,args=[str(source/'tests/runtime-fixture.py'),'{{port}}'])))
 first=rpc('model.start',{'id':'managed'});assert first['status']=='running'
 changed=rpc('model.get',{'id':'managed'});changed['endpoint']=f'http://127.0.0.1:{port+1}'
 denied=rpc('model.save',changed,ok=False);assert 'Stop' in str(denied) or 'stop' in str(denied),denied
 usage=rpc('instance.get',{'id':first['id']})['usage'];assert len(usage['process_ids'])>=2 and usage['committed_bytes']>0,usage
 stopped=rpc('model.stop',{'id':'managed'});assert stopped['status']=='stopped'
 completed=wait(send('managed'));assert completed['status']=='completed' and 'Managed runtime' in completed['content'],completed
 current=rpc('model.get',{'id':'managed'});assert current['instance_id']!=first['id']
 rpc('model.stop',{'id':'managed'})
 current=rpc('model.get',{'id':'managed'});current['engine_model']='slow';rpc('model.save',current)
 task=send('managed');rpc('model.stop',{'id':'managed'});assert rpc('run.get',{'id':task['id']})['status']=='cancelled'
 task=send('managed');runtime_id=rpc('model.get',{'id':'managed'})['instance_id'];rpc('instance.stop',{'id':runtime_id})
 assert rpc('run.get',{'id':task['id']})['status']=='cancelled'
 assert rpc('instance.get',{'id':runtime_id})['status']=='stopped'
 checks.append('Managed runtime starts with enforced resources, auto-starts on send, exposes descendant memory, and stops dependent tasks')
 probe=socket.socket();probe.bind(('127.0.0.1',0));second_port=probe.getsockname()[1];probe.close()
 rpc('model.save',dict(id='managed-second',name='Second managed fixture',protocol='ollama',endpoint=f'http://127.0.0.1:{second_port}',engine_model='slow',profile_id='cpu-diagnostics',context_tokens=2048,max_output_tokens=128,temperature=0,vision=False,runtime=dict(kind='command',command=sys.executable,args=[str(source/'tests/runtime-fixture.py'),'{{port}}'])))
 task_a=send('managed');task_b=send('managed-second')
 runtime_a=rpc('model.get',{'id':'managed'})['instance_id'];runtime_b=rpc('model.get',{'id':'managed-second'})['instance_id']
 assert runtime_a!=runtime_b
 assert rpc('instance.get',{'id':runtime_a})['status']=='running' and rpc('instance.get',{'id':runtime_b})['status']=='running'
 rpc('model.stop',{'id':'managed'})
 assert rpc('run.get',{'id':task_a['id']})['status']=='cancelled'
 assert rpc('instance.get',{'id':runtime_b})['status']=='running'
 independent=wait(task_b);assert independent['status']=='completed',independent
 rpc('model.stop',{'id':'managed-second'})
 checks.append('Two managed CPU fixture engines run on separate endpoints; stopping one cancels only its task while the second completes')
 task=send('slow');process.terminate();process.wait(10);start();result=rpc('run.get',{'id':task['id']});assert result['status']=='interrupted',result
 assert len(rpc('chat.list',{}))>=7
 checks.append('Controller restart preserves chats and marks unfinished work interrupted')
 full_events=rpc('events.list',{'after':0,'limit':500});assert len(full_events)>100
 snapshot=rpc('snapshot',{});recent=snapshot['events']
 assert len(recent)==100 and recent[0]['seq']>1 and recent[-1]['seq']>=full_events[-1]['seq']
 checks.append('Desktop snapshot shows the newest 100 events after a long session while cursor access retains the earlier journal')
 output={'ok':True,'checks':checks,'root':str(root)}
 (root/'agent-smoke-result.json').write_text(json.dumps(output,ensure_ascii=False,indent=2),encoding='utf-8');print(json.dumps(output,ensure_ascii=False))
finally:
 if process and process.poll() is None:process.terminate();process.wait(10)
 server.shutdown();server.server_close()
