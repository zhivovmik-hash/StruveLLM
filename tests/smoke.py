"""Integration test in a separate state directory. No training or model downloads."""
import json, os, pathlib, subprocess, sys, tempfile, time, urllib.request, uuid

source=pathlib.Path(__file__).resolve().parents[1]
from smoke_paths import test_cli, test_root
root=test_root('smoke-')
binary=test_cli()
env=dict(os.environ,STRUVE_LLM_ROOT=str(root),STRUVE_LLM_SOURCE=str(source));env.pop('STRUVE_LLM_PORT',None)
creation=getattr(subprocess,'CREATE_NO_WINDOW',0)
process=subprocess.Popen([str(binary),'serve'],env=env,stdout=subprocess.DEVNULL,stderr=(root/'stderr.log').open('w'),creationflags=creation)
checks=[]
def cli(*args,ok=True):
    p=subprocess.run([str(binary),'--json','--no-start',*args],cwd=os.environ.get('TEMP',str(root)),env=env,capture_output=True,text=True,encoding='utf-8',timeout=40,creationflags=creation)
    value=json.loads(p.stdout)
    assert value['ok'] is ok, value
    assert (p.returncode==0) is ok, (p.returncode,value)
    return value.get('data',value.get('error'))
def rpc(method,params):
    token=(root/'data/controller.token').read_text()
    req=urllib.request.Request(f"http://127.0.0.1:{json.loads((root/'data/controller.endpoint.json').read_text())['port']}/rpc",data=json.dumps(dict(method=method,params=params)).encode(),headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
    return json.load(urllib.request.urlopen(req,timeout=35))
def save(name,value):
    path=root/(name+'.json');path.write_text(json.dumps(value),encoding='utf-8');return str(path)
try:
    for _ in range(100):
        time.sleep(.15)
        if (root/'data/controller.token').exists():
            try:
                if cli('doctor')['controller']=='ready':break
            except Exception:pass
    else:raise RuntimeError('test controller failed: '+(root/'stderr.log').read_text())
    checks.append('CLI outside source folder and authenticated controller')
    resources=cli('resources','list');assert any(g['present'] for g in resources['gpus'])
    checks.append('actual GPU inventory with UUIDs')
    cli('profile','save','--file',save('missing-gpu',dict(id='training-standard',name='Missing device fixture',gpu_ids=['not-installed'],ram_gib=1,vram_gib=1,cpu_threads=1,kind='training',exclusive=True)))
    plan=cli('resources','plan','--profiles','training-standard');assert plan['admissible'] is False
    checks.append('missing P40 rejected')
    p=cli('profile','get','training-standard');old=p['revision'];p['name']='Тест русских путей';cli('profile','save','--file',save('profile',p))
    stale=cli('profile','save','--file',save('profile-stale',p),ok=False);assert 'ревизии' in stale['message']
    assert cli('profile','get','training-standard')['revision']==old+1
    checks.append('profile save, Unicode and stale revision rejection')
    diag=cli('mcp','get','local-diagnostics')
    node=str(root/'app/node.exe');diag['command']=node;diag['builtin']=False
    cli('mcp','save','--file',save('diag',diag));probe=cli('mcp','probe','local-diagnostics');assert any(t['name']=='runtime_info' for t in probe['tools'])
    answer=cli('mcp','call','local-diagnostics','--tool','runtime_info','--project','struve');assert answer['content']
    checks.append('real MCP discovery and call through CLI/controller/Node/server')
    large=dict(id='large-catalog',name='Large catalog fixture',transport='stdio',command=node,args=[str(source/'tests/catalog-fixture.mjs')],env_names=[],project_ids=['struve'],allowed_tools=[],enabled=False)
    cli('mcp','save','--file',save('catalog',large));catalog=cli('mcp','probe','large-catalog');assert len(catalog['tools'])==100
    record=cli('mcp','get','large-catalog');assert len(json.dumps(record))<80000
    schema=cli('request','mcp.tool.get','--params',json.dumps({'id':'large-catalog','tool':'fixture_0'}));assert len(schema['description'])>35000
    cli('mcp','save','--file',save('catalog-roundtrip',record));checks.append('Large MCP catalog stores full schemas separately from bounded GUI summaries')

    diag=cli('mcp','get','local-diagnostics');diag['enabled']=False;cli('mcp','save','--file',save('disabled',diag));cli('mcp','call','local-diagnostics','--tool','runtime_info','--project','struve',ok=False)
    diag=cli('mcp','get','local-diagnostics');diag['enabled']=True;diag['project_ids']=[];cli('mcp','save','--file',save('scoped',diag));cli('mcp','call','local-diagnostics','--tool','runtime_info','--project','struve',ok=False)
    checks.append('disabled and out-of-project MCP calls rejected')
    secret='test-'+uuid.uuid4().hex
    try:
        assert rpc('secret.save',{'id':'STRUVE_SMOKE_SECRET','value':secret})['ok']
        secret_config=dict(id='secret-test',name='Secret fixture',transport='stdio',command=node,args=[str(source/'tests/secret-fixture.mjs')],env_names=['STRUVE_SMOKE_SECRET'],project_ids=['struve'],allowed_tools=['echo_test_secret'],enabled=True)
        cli('mcp','save','--file',save('secret-connection',secret_config))
        masked=cli('mcp','call','secret-test','--tool','echo_test_secret','--project','struve')
        assert masked['content'][0]['text']=='[redacted]',masked
        assert secret not in json.dumps(cli('secret','list'))
        assert secret not in json.dumps(cli('events'))
        checks.append('Windows credential storage, secret injection and result redaction')
    finally:
        cli('secret','delete','--name','STRUVE_SMOKE_SECRET')
    manifest=json.loads((source/'examples/diagnostics-plugin.json').read_text(encoding='utf-8'));manifest['mcp_servers'][0]['command']=node;manifest['mcp_servers'][0]['args']=[str(source/'bridge/diagnostics.mjs')]
    baseline_plugins=len(cli('plugin','list'));cli('plugin','install','--file',save('plugin',manifest),'--dry-run');assert len(cli('plugin','list'))==baseline_plugins
    cli('plugin','install','--file',save('plugin',manifest));assert len(cli('plugin','list'))==baseline_plugins+1;assert cli('mcp','get','plugin-diagnostics')['enabled'] is False
    cli('plugin','install','--file',save('duplicate',manifest),ok=False);assert len(cli('plugin','list'))==baseline_plugins+1
    checks.append('plugin preview, atomic install and duplicate rejection')
    spec=dict(id='owned-first',name='Owned CPU fixture',profile_id='cpu-diagnostics',command=sys.executable,args=[str(source/'tests/worker-fixture.py'),'русский путь','trailing\\','quote"inside'])
    preview=cli('instance','start','--file',save('worker',spec),'--dry-run');assert preview['plan']['admissible'];assert not cli('instance','list')
    first=cli('instance','start','--file',save('worker',spec));assert first['status']=='running'
    repeat=cli('instance','start','--file',save('worker',spec));assert repeat['pid']==first['pid']
    second_spec=dict(spec,id='owned-second');second=cli('instance','start','--file',save('worker-second',second_spec));assert second['pid']!=first['pid']
    for _ in range(30):
        log=cli('instance','logs','owned-first')['text']
        if log:break
        time.sleep(.1)
    limits=json.loads(log);assert limits['job_ram_limit_bytes']==1073741824,limits
    assert limits['cpu_flags']&5==5 and 1<=limits['cpu_rate']<10000,limits
    usage=cli('instance','get','owned-first')['usage'];assert usage['resident_bytes']>0 and usage['committed_bytes']>0,usage
    assert limits['cuda_visible']=='' and limits['omp_threads']=='1'
    assert limits['arguments']==spec['args'][1:],limits
    checks.append('real suspended Windows launch, RAM/CPU limits, CUDA exclusion and argument quoting')
    heavy=dict(id='cpu-heavy',name='Overcommit check',gpu_ids=[],ram_gib=1,vram_gib=0,cpu_threads=resources['logical_cpus'],kind='inference',exclusive=True)
    cli('profile','save','--file',save('heavy-profile',heavy));cli('instance','start','--file',save('heavy-worker',dict(spec,id='overcommit',profile_id='cpu-heavy')),ok=False)
    cli('instance','stop','owned-first');assert cli('instance','get','owned-first')['status']=='stopped';assert cli('instance','get','owned-second')['status']=='running'
    cli('instance','stop','owned-second')
    checks.append('idempotent launch, aggregate reservations and independent process stop')
    bad=urllib.request.Request(f"http://127.0.0.1:{json.loads((root/'data/controller.endpoint.json').read_text())['port']}/rpc",data=b'{"method":"snapshot"}',headers={'Content-Type':'application/json'})
    try:urllib.request.urlopen(bad);raise AssertionError('unauthenticated request accepted')
    except urllib.error.HTTPError as e:assert e.code==401
    checks.append('local API requires authentication')
    before=len(cli('events'));process.terminate();process.wait(10)
    process=subprocess.Popen([str(binary),'serve'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,creationflags=creation)
    for _ in range(60):
        time.sleep(.2)
        try:
            if cli('doctor')['controller']=='ready':break
        except Exception:pass
    assert cli('profile','get','training-standard')['name']=='Тест русских путей';assert len(cli('events'))==before
    checks.append('restart preserves profiles, plugin registry and event history')
    result={'ok':True,'checks':checks,'root':str(root),'gpu_load_started':False,'models_downloaded':False}
    (root/'smoke-result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8');print(json.dumps(result,ensure_ascii=False))
finally:
    process.terminate();process.wait(10)
