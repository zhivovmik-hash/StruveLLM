"""A timed-out stdio integration must not leave its own grandchild running."""
import ctypes,json,os,pathlib,subprocess,sys,tempfile,time,urllib.request
base=pathlib.Path(__file__).resolve().parents[1]
from smoke_paths import test_cli, test_root
root=test_root('mcp-tree-')
binary=test_cli()
env=dict(os.environ,STRUVE_LLM_ROOT=str(root),STRUVE_LLM_SOURCE=str(base));env.pop('STRUVE_LLM_PORT',None)
controller=subprocess.Popen([binary,'serve'],env=env,stdout=subprocess.DEVNULL,stderr=(root/'stderr.log').open('w'),creationflags=subprocess.CREATE_NO_WINDOW)
kernel=ctypes.WinDLL('kernel32',use_last_error=True)
kernel.OpenProcess.argtypes=[ctypes.c_uint32,ctypes.c_int,ctypes.c_uint32];kernel.OpenProcess.restype=ctypes.c_void_p
kernel.WaitForSingleObject.argtypes=[ctypes.c_void_p,ctypes.c_uint32];kernel.CloseHandle.argtypes=[ctypes.c_void_p]
kernel.TerminateProcess.argtypes=[ctypes.c_void_p,ctypes.c_uint32]
def alive(pid):
 handle=kernel.OpenProcess(0x100000,0,pid)
 if not handle:return False
 try:return kernel.WaitForSingleObject(handle,0)==258
 finally:kernel.CloseHandle(handle)
def rpc(method,params):
 record=json.loads((root/'data/controller.endpoint.json').read_text());token=(root/'data/controller.token').read_text()
 req=urllib.request.Request(f"http://127.0.0.1:{record['port']}/rpc",data=json.dumps(dict(method=method,params=params)).encode(),headers={'Content-Type':'application/json','Authorization':'Bearer '+token})
 return json.load(urllib.request.urlopen(req,timeout=20))
child_pid=None
try:
 for _ in range(100):
  time.sleep(.1)
  try:
   if rpc('ping',{})['ok']:break
  except Exception:pass
 else:raise RuntimeError('Controller not ready')
 pid_file=root/'child.pid'
 connection=dict(id='stalled-tree',name='Owned timeout fixture',transport='stdio',command=sys.executable,args=[str(base/'tests/command-tree-fixture.py'),str(pid_file)],env_names=[],project_ids=['struve'],allowed_tools=[],enabled=False,timeout_seconds=1)
 assert rpc('mcp.save',connection)['ok'];started=time.time();result=rpc('mcp.probe',{'id':'stalled-tree'});assert not result['ok'],result
 assert pid_file.exists(),'Fixture did not start before timeout'
 child_pid=int(pid_file.read_text())
 for _ in range(30):
  if not alive(child_pid):break
  time.sleep(.1)
 leaked=alive(child_pid);record=dict(ok=not leaked,root=str(root),child_pid=child_pid,descendant_survived_timeout=leaked,elapsed_seconds=time.time()-started,error=result['error'])
 output=root/('mcp-tree-baseline.json' if '--expect-leak' in sys.argv else 'mcp-tree-result.json');output.write_text(json.dumps(record,indent=2),encoding='utf-8');print(json.dumps(record),flush=True)
 if '--expect-leak' in sys.argv:assert leaked,'Baseline did not reproduce the issue'
 else:assert not leaked,'Timed-out integration left its descendant running'
finally:
 if child_pid and alive(child_pid):
  handle=kernel.OpenProcess(1,0,child_pid)
  if handle:kernel.TerminateProcess(handle,1);kernel.CloseHandle(handle)
 controller.terminate();controller.wait(10)
