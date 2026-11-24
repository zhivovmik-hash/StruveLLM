"""Real isolated controller/Windows Job checks. No model quality claims from fixtures."""
import json,os,pathlib,subprocess,tempfile,time,socket,sys
source=pathlib.Path(__file__).resolve().parents[1]
from smoke_paths import test_cli, test_root, compute_gpu
root=test_root('media-catalog-')
binary=test_cli()
env=dict(os.environ,STRUVE_LLM_ROOT=str(root),STRUVE_LLM_SOURCE=str(source));env.pop('STRUVE_LLM_PORT',None)
process=None;checks=[]
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
def wait(task):
 for _ in range(250):
  run=rpc('run.get',{'id':task['id']})
  if run['status'] not in ('running','starting'):return run
  time.sleep(.15)
 raise AssertionError(run)
try:
 start();hw=rpc('resources.list');gpu=compute_gpu(hw,rpc('settings.get'))
 weight=root/'fixture.gguf';weight.write_bytes(b'not real weights')
 templates=rpc('media.templates');assert {m['architecture'] for m in templates}=={'sd15','sdxl','sd35','flux1','flux2-klein','minimax-h3'}
 saved=[]
 for i,m in enumerate(templates):
  m.update(id='fixture-'+m['architecture'],name=m['architecture'],enabled=False,command=sys.executable,model_path=str(weight),gpu_id=gpu['id'],ram_gib=1+i/4,vram_gib=.5,cpu_threads=1,timeout_seconds=60,width=256,height=256)
  for field in ('llm_path','vae_path','clip_l_path','clip_g_path','t5_path'):m[field]=str(weight)
  saved.append(rpc('media.save',m))
 assert len({m['profile_id'] for m in saved})==6
 assert [rpc('profile.get',{'id':m['profile_id']})['ram_gib'] for m in saved]==[1+i/4 for i in range(6)]
 checks.append('Six architecture templates create independent model/resource records')
 m=saved[4];original=rpc('profile.get',{'id':m['profile_id']});rpc('media.save',dict(m,revision=0,ram_gib=22),ok=False);assert rpc('profile.get',{'id':m['profile_id']})==original
 rpc('profile.save',dict(original,ram_gib=22),ok=False)
 rpc('media.save',dict(m,enabled=True,gpu_id='GPU-absent'),ok=False);assert rpc('media.get',{'id':m['id']})==m
 rpc('media.check',dict(m,llm_path=''),ok=False)
 checks.append('Revision conflict, absent GPU and missing encoder reject without changing model/profile; direct profile bypass rejected')
 for i in (0,4,5):saved[i]=rpc('media.save',dict(saved[i],enabled=True))
 d=rpc('media.defaults.get');d=rpc('media.defaults.save',dict(d,image=saved[0]['id'],edit=saved[4]['id'],video=saved[5]['id']))
 rpc('media.defaults.save',dict(d,image=saved[5]['id']),ok=False);rpc('media.defaults.save',dict(d,revision=0),ok=False)
 rpc('media.archive',dict(id=saved[4]['id'],revision=saved[4]['revision']),ok=False)
 checks.append('Default roles are kind-checked and versioned; an active default cannot be archived')
 sock=socket.socket();sock.bind(('127.0.0.1',0));port=sock.getsockname()[1];sock.close()
 rpc('model.save',dict(id='planner',name='Fixture planner',protocol='ollama',endpoint=f'http://127.0.0.1:{port}',engine_model='plain',profile_id='cpu-diagnostics',context_tokens=2048,max_output_tokens=128,temperature=0,runtime=dict(kind='command',command=sys.executable,args=[str(source/'tests/runtime-fixture.py'),'{{port}}',str(root/'planner-requests.jsonl')])))
 for mode,kind,expected in [('image','image',saved[0]),('video','video',saved[5])]:
  task=rpc('chat.send',dict(project_id='struve',model_id='planner',mode=mode,content='Draw a telescope'))
  run=wait(task);assert run['kind']==kind and run['status']=='failed' and 'exited with code' in run['error'],run
  assert run['execution']['image_config']['id']==expected['id'] and run['execution']['profile']['id']==expected['profile_id']
  assert run['planner_kept_loaded'] and not run['llm_unloaded'] and not run.get('artifacts')
  assert rpc('chat.get',{'id':run['chat_id']})['messages'][-1]['role']=='user'
  workers=[i for i in rpc('instance.list') if i['id'].startswith(run['id'])];assert all(i['profile_id']==expected['profile_id'] for i in workers)
  rpc('model.stop',{'id':'planner'})
 checks.append('Image and video chat dispatch use selected immutable profile snapshots; real failed workers restore LLM and publish no fictional files')
 # First/last references reach the real controller and renderer contract, at
 # native 480p and an exact 15-second target. The fixture still fails rendering.
 from PIL import Image
 refs=[]
 for i,color in enumerate(('red','blue')):
  path=root/f'reference-{i}.png';Image.new('RGB',(256,256),color).save(path)
  refs.append(rpc('attachment.import',{'path':str(path)}))
 planner=rpc('model.get',{'id':'planner'});rpc('model.save',dict(planner,vision=True,context_tokens=4096))
 video=rpc('media.get',{'id':saved[5]['id']})
 rpc('media.save',dict(video,sampling_schedule='hyperflow-h3-flash-6',steps=20),ok=False)
 for invalid in (-1, 65537, 2.5, '512'):
  rpc('media.save',dict(video,h3_linear_chunk_tokens=invalid),ok=False)
 rpc('media.save',dict(saved[4],h3_linear_chunk_tokens=512),ok=False)
 progress_fixture=root/'progress-fixture.exe'
 subprocess.run(['rustc','--edition=2021',str(source/'tests/h3-progress-fixture.rs'),'-o',str(progress_fixture)],check=True,timeout=90)
 rpc('media.save',dict(video,command=str(progress_fixture),width=864,height=480,diffusion_cpu=True,h3_linear_chunk_tokens=512,sampling_schedule='hyperflow-h3-flash-6',steps=6,sampling_method='euler',cfg_scale=1))
 assert rpc('media.get',{'id':video['id']})['h3_linear_chunk_tokens']==512
 params=dict(project_id='struve',model_id='planner',mode='video',content='Create video for 15 seconds',attachment_ids=[f['id'] for f in refs])
 rpc('chat.send',dict(params,attachment_ids=[refs[0]['id']]*3),ok=False)
 planner=rpc('model.get',{'id':'planner'});rpc('model.save',dict(planner,max_attachment_files=1))
 rpc('chat.send',params,ok=False)
 planner=rpc('model.get',{'id':'planner'});rpc('model.save',dict(planner,max_attachment_files=2))
 task=rpc('chat.send',params)
 seen=[];deadline=time.monotonic()+45
 while time.monotonic()<deadline:
  run=rpc('run.get',{'id':task['id']})
  progress=run.get('render_progress')
  if progress and progress not in seen:seen.append(progress)
  if run['status'] not in ('running','starting'):break
  time.sleep(.1)
 assert [(p['stage'],p['steps_completed'],p['steps_total']) for p in seen]==[
  ('conditioning',0,6),('diffusion',0,6),('diffusion',1,6),('vae_decode',6,6),('writing_video',6,6)],seen
 checks.append('Live run.get reports H3 conditioning, real sampler units, decode and writing; skips tensor-loading and encoder bars. Test worker deliberately exits with an error.')
 assert run['status']=='failed' and run['planner_kept_loaded'] and not run['llm_unloaded'] and not run.get('artifacts'),run
 assert run['target_video_frames']==360 and run['effective_media_config']['video_frames']==362,run
 assert run['source']['attachment']['id']==refs[0]['id'] and run['video_reference_end']['attachment']['id']==refs[1]['id']
 args=run['render_command']['args'];assert args[args.index('--init-img')+1]==refs[0]['path'] and args[args.index('--end-img')+1]==refs[1]['path']
 assert args[args.index('-W')+1]=='864' and args[args.index('-H')+1]=='480'
 assert args[args.index('--backend')+1]=='diffusion=cpu' and '--offload-to-cpu' in args
 assert args[args.index('--sigmas')+1]=='hyperflow-h3-flash-6' and args[args.index('--steps')+1]=='6'
 assert args[args.index('--max-vram')+1]=='0.5'
 assert args[args.index('--model-args')+1]=='minimax_h3_linear_chunk_tokens=512'
 request=json.loads((root/'planner-requests.jsonl').read_text().splitlines()[-1]);assert request['image_count']==2 and request['format'] is None,request
 rpc('model.stop',{'id':'planner'})
 checks.append('Two ordered image references reach first/last frame renderer arguments at 864x480; 15 seconds maps to 362 native/360 target frames; excess references reject; saved CPU/RAM diffusion mode reaches actual worker arguments')
 checks.append('HyperFlow step mismatch rejects; saved six-step schedule and VRAM weight budget reach actual worker arguments')
 m=rpc('media.get',{'id':saved[4]['id']})
 assert rpc('media.check',m)['selected_gpu_assignments']==[]
 holder=rpc('instance.start',dict(id='catalog-queue-holder',profile_id=m['profile_id'],command=sys.executable,args=['-c','import time; time.sleep(90)']))
 assert rpc('media.check',m)['selected_gpu_assignments']==[dict(instance_id=holder['id'],pid=holder['pid'])]
 pending=rpc('chat.send',dict(project_id='struve',model_id='planner',mode='image',media_model_id=m['id'],content='Draw a telescope'))
 assert pending['status']=='queued'
 rpc('media.save',dict(m,enabled=False,gpu_id='GPU-removed',model_path='missing'))
 rpc('instance.stop',{'id':holder['id']})
 assert rpc('media.check',m)['selected_gpu_assignments']==[]
 checks.append('File and profile validation reports current Struve GPU assignments by instance and PID, including their release')
 for _ in range(100):
  rejected=rpc('run.get',{'id':pending['id']})
  if rejected['status']!='queued':break
  time.sleep(.15)
 assert rejected['status']=='failed' and 'disabled' in rejected['error'] and 'worker_id' not in rejected
 checks.append('Disabling a registered media model while queued rejects the stale task before a worker starts')
 rpc('chat.send',dict(project_id='struve',model_id='planner',mode='image',media_model_id=m['id'],content='Draw'),ok=False)
 checks.append('Disabled models retain editable settings on changed hardware and cannot execute')
 process.terminate();process.wait(10);start();assert len(rpc('media.list'))==6;assert rpc('media.defaults.get')==d
 result=dict(ok=True,version=rpc('doctor')['version'],checks=checks,root=str(root),scope='Registry/resource lifecycle; intentionally invalid renderer, not generation quality')
 (root/'media-catalog-result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result))
finally:
 if process and process.poll() is None:process.terminate();process.wait(10)
