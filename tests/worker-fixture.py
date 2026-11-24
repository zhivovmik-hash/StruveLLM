"""Read the actual Windows job limits, then sleep; no GPU or training work."""
import ctypes, json, os, sys, time
from ctypes import wintypes
class Basic(ctypes.Structure):
    _fields_=[('process_time',ctypes.c_int64),('job_time',ctypes.c_int64),('flags',wintypes.DWORD),('min_working_set',ctypes.c_size_t),('max_working_set',ctypes.c_size_t),('active_limit',wintypes.DWORD),('affinity',ctypes.c_size_t),('priority',wintypes.DWORD),('scheduling',wintypes.DWORD)]
class Io(ctypes.Structure):
    _fields_=[(name,ctypes.c_uint64) for name in ['read_count','write_count','other_count','read_bytes','write_bytes','other_bytes']]
class Extended(ctypes.Structure):
    _fields_=[('basic',Basic),('io',Io),('process_memory',ctypes.c_size_t),('job_memory',ctypes.c_size_t),('peak_process',ctypes.c_size_t),('peak_job',ctypes.c_size_t)]
class Cpu(ctypes.Structure):
    _fields_=[('flags',wintypes.DWORD),('rate',wintypes.DWORD)]
kernel=ctypes.WinDLL('kernel32',use_last_error=True)
info=Extended();cpu=Cpu()
assert kernel.QueryInformationJobObject(None,9,ctypes.byref(info),ctypes.sizeof(info),None)
assert kernel.QueryInformationJobObject(None,15,ctypes.byref(cpu),ctypes.sizeof(cpu),None)
print(json.dumps(dict(pid=os.getpid(),job_ram_limit_bytes=info.job_memory,cpu_rate=cpu.rate,cpu_flags=cpu.flags,cuda_visible=os.environ.get('CUDA_VISIBLE_DEVICES'),omp_threads=os.environ.get('OMP_NUM_THREADS'),arguments=sys.argv[1:])),flush=True)
time.sleep(45)
