"""Failure/cancellation/recovery contract on a real controller and Windows Jobs.
The planner is a local HTTP fixture and Python is an intentionally invalid renderer.
Actual CUDA image generation is qualified separately with the installed real models.
"""
import json,os,pathlib,subprocess,tempfile,time,socket,sys
source=pathlib.Path(__file__).resolve().parents[1]
from smoke_paths import test_cli, test_root, compute_gpu
root=test_root('image-workflow-')
binary=test_cli()
env=dict(os.environ,STRUVE_LLM_ROOT=str(root),STRUVE_LLM_SOURCE=str(source));env.pop('STRUVE_LLM_PORT',None)
checks=[];process=None
def rpc(method,params={},ok=True):
 import urllib.request
 req=urllib.request.Request(endpoint,data=json.dumps(dict(method=method,params=params)).encode(),headers={'Content-Type':'application/json','Authorization':'Bearer '+token})
 value=json.load(urllib.request.urlopen(req,timeout=45));assert value['ok']==ok,value
 return value.get('data',value.get('error'))
def start():
 global process,endpoint,token
 process=subprocess.Popen([str(binary),'serve'],env=env,stdout=subprocess.DEVNULL,stderr=(root/'stderr.log').open('a'),creationflags=subprocess.CREATE_NO_WINDOW)
 for _ in range(100):
  time.sleep(.1)
  try:
   record=json.loads((root/'data/controller.endpoint.json').read_text());endpoint=f"http://127.0.0.1:{record['port']}/rpc";token=(root/'data/controller.token').read_text()
   if rpc('ping')['pid']==process.pid:return
  except Exception:pass
 raise RuntimeError((root/'stderr.log').read_text())
def wait(task,phase=None):
 for _ in range(250):
  r=rpc('run.get',{'id':task['id']})
  if r['status'] not in ['running','starting','queued'] or (phase and r['phase']==phase):return r
  time.sleep(.15)
 raise AssertionError(r)
def send(**extra):return rpc('chat.send',dict(project_id='struve',model_id='planner',mode='image',content='Draw a telescope under a starry sky.',**extra))
def modify_model(**extra):
 rpc('model.stop',{'id':'planner'});m=rpc('model.get',{'id':'planner'});m.update(extra);return rpc('model.save',m)
try:
 start();hw=rpc('resources.list');gpu=compute_gpu(hw,rpc('settings.get'))
 cfg=rpc('image.config.get');assert cfg['enabled']==False and cfg['gpu_id']==''
 file=root/'fixture.safetensors';file.write_bytes(b'intentionally invalid model fixture')
 cfg.update(enabled=True,command=sys.executable,model_path=str(file),gpu_id=gpu['id'],ram_gib=.5,vram_gib=.5,cpu_threads=1,width=256,height=256,steps=1,seed=42,timeout_seconds=60)
 cfg=rpc('image.config.save',cfg);original_profile=rpc('profile.get',{'id':'image-generation'})
 for field,value in [('gpu_id','GPU-absent'),('width',513),('steps',0),('cfg_scale',30),('seed',-2),('cpu_threads',999),('model_path',str(root/'missing.safetensors'))]:
  invalid=dict(cfg);invalid[field]=value;rpc('image.config.save',invalid,ok=False)
 assert rpc('image.config.get')==cfg
 stale=dict(cfg,revision=0,ram_gib=1);rpc('image.config.save',stale,ok=False);assert rpc('profile.get',{'id':'image-generation'})==original_profile
 checks.append('Absent GPU, invalid parameters and stale configuration reject without changing the coupled resource profile')
 s=socket.socket();s.bind(('127.0.0.1',0));port=s.getsockname()[1];s.close()
 model=rpc('model.save',dict(id='planner',name='Planner fixture',protocol='ollama',endpoint=f'http://127.0.0.1:{port}',engine_model='plain',profile_id='image-generation',context_tokens=2048,max_output_tokens=128,temperature=0,runtime=dict(kind='command',command=sys.executable,args=[str(source/'tests/runtime-fixture.py'),'{{port}}'])))
 rpc('chat.send',dict(project_id='struve',model_id='planner',mode='image',content='draw',attachment_ids=['unsupported']),ok=False)
 task=send();r=wait(task);assert r['status']=='failed' and 'exited with code' in r['error'],r
 assert r['llm_restored'] and r['llm_unloaded'];assert [s['phase'] for s in r['stages']]==['planning','unloading','rendering','restoring','done']
 assert not rpc('chat.get',{'id':r['chat_id']})['messages'][-1]['role']=='assistant'
 assert rpc('model.stop',{'id':'planner'})['status']=='stopped'
 checks.append('Renderer failure after actual process handoff delivers no fake artifact, restores planner and permits normal model stop afterward')
 modify_model(engine_model='slow')
 task=send();time.sleep(.5)
 assert 'owns' in rpc('model.start',{'id':'planner'},ok=False)['message']
 competing=rpc('chat.send',dict(project_id='struve',model_id='planner',content='other task'))
 assert competing['status']=='queued' and competing['wait_reason']['owner_id']==task['id'],competing
 rpc('run.cancel',{'id':competing['id']})
 rpc('image.config.save',cfg,ok=False)
 rpc('run.finish',dict(id=task['id'],content='fake success'),ok=False)
 rpc('instance.start',dict(id='conflict',name='Conflict',profile_id='image-generation',command=sys.executable,args=['--version'],image_owner=task['id']),ok=False)
 rpc('run.cancel',{'id':task['id']});r=wait(task);assert r['status']=='cancelled' and r['llm_restored'],r
 assert len([i for i in rpc('instance.list') if i['status']=='running'])==1
 checks.append('Cancellation interrupts a live planner process; competing launches, settings changes, forged ownership and external completion are refused until cleanup')
 modify_model(agent_timeout_seconds=1)
 task=send();r=wait(task);assert r['status']=='failed' and r['llm_restored'],r
 checks.append('Planner uses its configured one-second deadline and returns the runtime after timeout')
 modify_model(agent_timeout_seconds=30)
 task=send();time.sleep(.5);process.terminate();process.wait(10);start()
 r=rpc('run.get',{'id':task['id']});assert r['status']=='interrupted',r
 assert not [i for i in rpc('instance.list') if i['status']=='running']
 checks.append('Controller crash marks the durable image task interrupted, kills its owned process tree and does not replay generation')
 modify_model(engine_model='plain')
 r=wait(send());assert r['status']=='failed' and r['llm_restored'],r
 checks.append('New work is admitted after restart; an orphan resource lease cannot block the next request')
 rpc('model.stop',{'id':'planner'})
 # Reserve the selected GPU with a managed test process. It consumes no CUDA memory.
 holder=rpc('instance.start',dict(id='queue-holder',profile_id='image-generation',command=sys.executable,args=['-c','import time; time.sleep(90)']))
 first=send();second=send();third=send()
 assert all(t['status']=='queued' for t in (first,second,third))
 rpc('chat.send',dict(project_id='struve',model_id='planner',chat_id=first['chat_id'],mode='image',content='Draw'),ok=False)
 assert rpc('run.cancel',{'id':second['id']})['status']=='cancelled'
 for task in [first,third]:
  waiting=rpc('run.get',{'id':task['id']});assert waiting['phase']=='waiting_for_gpu' and 'worker_id' not in waiting
 assert rpc('instance.get',{'id':holder['id']})['status']=='running'
 rpc('instance.stop',{'id':holder['id']})
 a=wait(first);b=wait(third)
 assert all(r['status']=='failed' and r['llm_restored'] and r['stages'][0]['phase']=='waiting_for_gpu' for r in [a,b]),(a,b)
 assert a['completed_at']<=b['started_at'] and not rpc('run.get',{'id':second['id']}).get('worker_id')
 assert len(rpc('chat.get',{'id':first['chat_id']})['messages'])==1
 checks.append('Two queued requests retain FIFO order, an independently owned GPU process remains live, and a cancelled middle request creates no worker or artifact')
 rpc('model.stop',{'id':'planner'})
 holder=rpc('instance.start',dict(id='queue-restart-holder',profile_id='image-generation',command=sys.executable,args=['-c','import time; time.sleep(90)']))
 pending=send();assert pending['status']=='queued'
 process.terminate();process.wait(10);start()
 recovered=wait(pending);assert recovered['id']==pending['id'] and recovered['status']=='failed' and recovered['llm_restored']
 assert sum(s['phase']=='planning' for s in recovered['stages'])==1
 assert rpc('instance.get',{'id':holder['id']})['status']=='interrupted'
 checks.append('A pending request survives controller restart and starts once; in-flight generation is still interrupted and never replayed')
 rpc('model.stop',{'id':'planner'})
 holder=rpc('instance.start',dict(id='queue-change-holder',profile_id='image-generation',command=sys.executable,args=['-c','import time; time.sleep(90)']))
 pending=send();assert pending['status']=='queued'
 modify_model(agent_timeout_seconds=45)
 rpc('instance.stop',{'id':holder['id']})
 changed=wait(pending)
 assert changed['status']=='failed' and 'Planner settings changed' in changed['error'] and 'worker_id' not in changed
 checks.append('Changing the planner deadline while queued rejects the stale request before starting any worker')
 cfg=rpc('image.config.get');cfg.update(enabled=False,gpu_id='GPU-removed',model_path='absent');rpc('image.config.save',cfg)
 message=rpc('chat.send',dict(project_id='struve',model_id='planner',mode='image',content='draw'),ok=False)['message']
 assert 'enable' in message or 'disabled' in message
 checks.append('Generation can be disabled even after hardware or model files are removed')
 result=dict(ok=True,version=rpc('doctor')['version'],checks=checks,root=str(root),gpu_discovered=gpu['id'],renderer='intentional invalid Python fixture; no CUDA generation claimed')
 (root/'image-workflow-result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8');print(json.dumps(result,ensure_ascii=False))
finally:
 if process and process.poll() is None:process.terminate();process.wait(10)
