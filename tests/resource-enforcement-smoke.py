"""Empirically verify RAM denial and aggregate CPU rate for owned descendants."""
import datetime,json,os,pathlib,subprocess,sys,tempfile,time,urllib.request
source=pathlib.Path(__file__).resolve().parents[1]
from smoke_paths import test_cli, test_root
root=test_root('resource-cap-', with_node=False)
binary=test_cli()
env=dict(os.environ,STRUVE_LLM_ROOT=str(root),STRUVE_LLM_SOURCE=str(source));env.pop('STRUVE_LLM_PORT',None)
controller=subprocess.Popen([binary,'serve'],env=env,stdout=subprocess.DEVNULL,stderr=(root/'stderr.log').open('w'),creationflags=subprocess.CREATE_NO_WINDOW)
def rpc(method,params={}):
    record=json.loads((root/'data/controller.endpoint.json').read_text());token=(root/'data/controller.token').read_text()
    request=urllib.request.Request(f"http://127.0.0.1:{record['port']}/rpc",data=json.dumps(dict(method=method,params=params)).encode(),
        headers={'Content-Type':'application/json','Authorization':'Bearer '+token})
    result=json.load(urllib.request.urlopen(request,timeout=25));assert result['ok'],result;return result['data']
def start(identifier,mode):
    return rpc('instance.start',dict(id=identifier,name=identifier,profile_id='empirical-cap',command=sys.executable,args=[str(source/'tests/resource-limit-fixture.py'),mode]))
def read_result(identifier):
    deadline=time.monotonic()+45
    while time.monotonic()<deadline:
        log=rpc('instance.logs',{'id':identifier})['text'].strip()
        if log:
            try:return json.loads(log)
            except ValueError:
                if 'Traceback' in log:raise AssertionError(log)
        time.sleep(.5)
    raise AssertionError('Fixture did not finish: '+identifier)
try:
    for _ in range(100):
        time.sleep(.1)
        try:
            if rpc('ping')['pid']==controller.pid:break
        except Exception:pass
    else:raise RuntimeError('Controller did not start')
    rpc('profile.save',dict(id='empirical-cap',name='Empirical 256 MiB / 1 CPU',gpu_ids=[],ram_gib=.25,vram_gib=0,cpu_threads=1,kind='inference',exclusive=True))
    start('memory-cap','memory');memory=read_result('memory-cap');memory_usage=rpc('instance.get',{'id':'memory-cap'})['usage']
    assert all(child['blocked'] for child in memory['children']),memory
    assert all(0<child['allocated_bytes']<256*1024*1024 for child in memory['children']),memory
    rpc('instance.stop',{'id':'memory-cap'})
    start('cpu-cap','cpu');cpu=read_result('cpu-cap');cpu_usage=rpc('instance.get',{'id':'cpu-cap'})['usage']
    cpu['average_logical_cores']=sum(child['cpu_seconds'] for child in cpu['children'])/cpu['wall_seconds']
    assert .2<cpu['average_logical_cores']<1.35,cpu
    rpc('instance.stop',{'id':'cpu-cap'})
    receipt=dict(ok=True,at=datetime.datetime.now(datetime.timezone.utc).isoformat(),root=str(root),
        checks=['The configured 256 MiB Job allocation limit rejected further allocation in both descendants',
        'Four CPU consumers shared the one-logical-CPU hard rate cap'],memory=memory,memory_usage=memory_usage,cpu=cpu,cpu_usage=cpu_usage,gpu_work=False,
        peak_note='Peak accounting is recorded as returned by Windows; it is not used as proof that every peak counter stays below the allocation cap.')
    (root/'resource-enforcement-result.json').write_text(json.dumps(receipt,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps(receipt,ensure_ascii=False))
finally:
    controller.terminate();controller.wait(10)
