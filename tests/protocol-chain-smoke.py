"""Installed controller -> Job worker -> OpenAI SSE -> authenticated HTTP MCP -> chat."""
from pathlib import Path
import base64,json,os,secrets,subprocess,tempfile,threading,time,urllib.request
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler

source=Path(__file__).resolve().parents[1]
from smoke_paths import test_cli, test_root
root=test_root('protocol-chain-')
binary=test_cli()
node=root/'app/node.exe'
env=dict(os.environ,STRUVE_LLM_ROOT=str(root),STRUVE_LLM_SOURCE=str(source),STRUVE_HTTP_FIXTURE_TOKEN=secrets.token_urlsafe(24),STRUVE_HTTP_WRONG_TOKEN=secrets.token_urlsafe(24));env.pop('STRUVE_LLM_PORT',None)
requests=[];checks=[];controller=None;mcp=None
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_POST(self):
  payload=json.loads(self.rfile.read(int(self.headers['Content-Length'])));requests.append(dict(path=self.path,payload=payload))
  self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
  def event(value):
   encoded=('data: '+json.dumps(value,ensure_ascii=False)+'\n\n').encode('utf-8')
   for i in range(0,len(encoded),3):self.wfile.write(encoded[i:i+3])
   self.wfile.flush()
  try:
   if payload['model']=='tool' and payload['messages'][-1]['role']!='tool':
    event({'choices':[{'delta':{'tool_calls':[{'index':0,'id':'call_fixture','type':'function','function':{'name':'tool_0','arguments':'{"value":'}}]}}]})
    event({'choices':[{'delta':{'tool_calls':[{'index':0,'function':{'arguments':'"Пример ✓"}'}}]}}]})
    event({'choices':[{'delta':{},'finish_reason':'tool_calls'}]})
   else:
    answer='Ответ через OpenAI ✓' if payload['model']=='plain' else 'Результат HTTP-инструмента принят ✓' if payload['model']=='tool' else 'Изображение получено транспортом'
    for word in answer.split(' '):event({'choices':[{'delta':{'content':word+' '}}]})
    event({'choices':[{'delta':{},'finish_reason':'stop'}]})
    event({'choices':[],'usage':{'prompt_tokens':120,'completion_tokens':8}})
   self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush()
  except (ConnectionAbortedError,ConnectionResetError,BrokenPipeError):pass

listener=ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=listener.serve_forever,daemon=True).start()
def rpc(method,params=None,ok=True):
 record=json.loads((root/'data/controller.endpoint.json').read_text());token=(root/'data/controller.token').read_text()
 request=urllib.request.Request(f"http://127.0.0.1:{record['port']}/rpc",data=json.dumps(dict(method=method,params=params or {})).encode(),headers={'Content-Type':'application/json','Authorization':'Bearer '+token})
 value=json.load(urllib.request.urlopen(request,timeout=50));assert value['ok']==ok,value
 return value.get('data',value.get('error'))
def wait(task):
 for _ in range(200):
  run=rpc('run.get',dict(id=task['id']))
  if run['status'] not in ['running','starting']:return run
  time.sleep(.1)
 raise AssertionError('Task did not finish')
def send(model,**extra):return rpc('chat.send',dict(project_id='struve',model_id=model,content='Проверь транспорт на учебной фикстуре.',**extra))
try:
 mcp=subprocess.Popen([str(node),str(source/'tests/http-mcp-fixture.mjs'),str(root/'http-counts.json')],env=env,stdout=subprocess.PIPE,stderr=(root/'http-stderr.log').open('w'),text=True,encoding='utf-8',creationflags=subprocess.CREATE_NO_WINDOW)
 port=json.loads(mcp.stdout.readline())['port']
 controller=subprocess.Popen([str(binary),'serve'],env=env,stdout=subprocess.DEVNULL,stderr=(root/'controller-stderr.log').open('w'),creationflags=subprocess.CREATE_NO_WINDOW)
 for _ in range(100):
  time.sleep(.1)
  try:
   if rpc('ping')['pid']==controller.pid:break
  except Exception:pass
 else:raise RuntimeError('Controller not ready')
 config=dict(id='http-fixture',name='Authenticated HTTP fixture',transport='http',url=f'http://127.0.0.1:{port}/mcp',bearer_env='STRUVE_HTTP_FIXTURE_TOKEN',env_names=[],allowed_tools=['echo'],project_ids=['struve'],enabled=True,timeout_seconds=10)
 rpc('mcp.save',config)
 probe=rpc('mcp.probe',dict(id=config['id']));assert probe['server']['name']=='authenticated-http-fixture'
 manual=rpc('mcp.call',dict(id=config['id'],project_id='struve',tool='echo',arguments={'value':'Ручной вызов ✓'}))
 value=json.loads(manual['content'][0]['text']);assert value==dict(value='Ручной вызов ✓',fixture_secret='[redacted]'),value
 assert env['STRUVE_HTTP_FIXTURE_TOKEN'] not in json.dumps(manual)
 checks.append('Installed controller discovers authenticated Streamable HTTP tools, dispatches UTF-8 arguments, and redacts a fixture secret echoed by the server')
 bad=dict(config,id='bad-auth',bearer_env='STRUVE_HTTP_WRONG_TOKEN');rpc('mcp.save',bad)
 error=rpc('mcp.probe',dict(id='bad-auth'),ok=False);assert 'unauthorized' in str(error).lower(),error
 assert json.loads((root/'http-counts.json').read_text())['denied']>0
 checks.append('HTTP 401 is reported as a connection failure')
 config=rpc('mcp.get',dict(id='http-fixture'));config['project_ids']=[];rpc('mcp.save',config)
 before=json.loads((root/'http-counts.json').read_text())['calls']
 rpc('mcp.call',dict(id='http-fixture',project_id='struve',tool='echo',arguments={'value':'must not run'}),ok=False)
 assert json.loads((root/'http-counts.json').read_text())['calls']==before
 config=rpc('mcp.get',dict(id='http-fixture'));config['project_ids']=['struve'];rpc('mcp.save',config)
 checks.append('Project denial prevents the HTTP tool from executing')
 for name in ['plain','tool','vision']:
  rpc('model.save',dict(id=name,name=name,protocol='openai',endpoint=f'http://127.0.0.1:{listener.server_port}/v1',engine_model=name,profile_id='cpu-diagnostics',context_tokens=4096,max_output_tokens=128,temperature=0,vision=name=='vision',tool_mode='native'))
 task=send('plain');run=wait(task);assert run['status']=='completed',run
 assert rpc('chat.get',dict(id=task['chat_id']))['messages'][-1]['content'].strip()=='Ответ через OpenAI ✓'
 assert run['usage']==dict(input_tokens=120,output_tokens=8)
 assert requests[-1]['path']=='/v1/chat/completions'
 checks.append('OpenAI SSE reaches durable chat with Cyrillic byte fragmentation, finish reason and usage')
 task=send('tool',use_tools=True,tool_selection=[dict(connection_id='http-fixture',tool='echo')]);run=wait(task);assert run['status']=='completed',run
 assert len(run['steps'])==1 and run['steps'][0]['arguments']=={'value':'Пример ✓'} and run['steps'][0]['status']=='completed',run
 conversation=requests[-1]['payload']['messages'];assert conversation[-1]['role']=='tool' and conversation[-1]['tool_call_id']=='call_fixture'
 assert '[redacted]' in conversation[-1]['content'] and env['STRUVE_HTTP_FIXTURE_TOKEN'] not in json.dumps(conversation)
 assert run['content'].strip()=='Результат HTTP-инструмента принят ✓'
 checks.append('Fragmented native OpenAI tool call executes HTTP MCP once and returns a redacted, correlated tool receipt to the model')
 image=root/'pixel.png';image.write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aH3sAAAAASUVORK5CYII='))
 attachment=rpc('attachment.import',dict(path=str(image)));run=wait(send('vision',attachment_ids=[attachment['id']]));assert run['status']=='completed',run
 parts=requests[-1]['payload']['messages'][-1]['content'];assert any(p.get('type')=='image_url' and p['image_url']['url'].startswith('data:image/png;base64,') for p in parts),parts
 checks.append('Imported image reaches the OpenAI-compatible endpoint as a data URL; this checks transport, not vision quality')
 record=dict(ok=True,root=str(root),checks=checks,application_version=rpc('doctor')['version'],fixture=True,scientific_training_started=False)
 (root/'protocol-chain-result.json').write_text(json.dumps(record,ensure_ascii=False,indent=2),encoding='utf-8');print(json.dumps(record,ensure_ascii=False),flush=True)
finally:
 for process in [controller,mcp]:
  if process and process.poll() is None:process.terminate();process.wait(10)
 listener.shutdown();listener.server_close()
