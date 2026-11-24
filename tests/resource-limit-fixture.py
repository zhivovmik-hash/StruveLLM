"""Small bounded consumers for empirical Windows Job enforcement checks."""
import ctypes,json,os,subprocess,sys,time

mode=sys.argv[1]
if mode=='memory-child':
    class Memory(ctypes.Structure):
        _fields_=[('committed',ctypes.c_uint64),('peak',ctypes.c_uint64)]
    kernel=ctypes.WinDLL('kernel32',use_last_error=True)
    def memory():
        value=Memory();assert kernel.QueryInformationJobObject(None,28,ctypes.byref(value),ctypes.sizeof(value),None)
        return dict(committed=value.committed,peak=value.peak)
    blocks=[];blocked=False;samples=[]
    try:
        for _ in range(128):blocks.append(bytearray(4*1024*1024));samples.append(memory());time.sleep(.005)
    except MemoryError:blocked=True
    print(json.dumps(dict(pid=os.getpid(),blocked=blocked,allocated_bytes=len(blocks)*4*1024*1024,
        after_failure=memory(),maximum_observed_committed_bytes=max(s['committed'] for s in samples))),flush=True)
elif mode=='cpu-child':
    begin=time.process_time();deadline=time.perf_counter()+12;value=1
    while time.perf_counter()<deadline:
        for _ in range(5000):value=(value*1664525+1013904223)&0xffffffff
    print(json.dumps(dict(pid=os.getpid(),cpu_seconds=time.process_time()-begin)),flush=True)
else:
    count=2 if mode=='memory' else min(4,os.cpu_count() or 1)
    started=time.perf_counter()
    children=[subprocess.Popen([sys.executable,__file__,mode+'-child'],stdout=subprocess.PIPE,stderr=subprocess.PIPE,
        text=True,encoding='utf-8',creationflags=subprocess.CREATE_NO_WINDOW) for _ in range(count)]
    results=[]
    for child in children:
        stdout,stderr=child.communicate(timeout=35)
        if child.returncode:raise RuntimeError(f'Child {child.pid}: {stderr}')
        results.append(json.loads(stdout))
    print(json.dumps(dict(mode=mode,wall_seconds=time.perf_counter()-started,children=results)),flush=True)
    time.sleep(20)
