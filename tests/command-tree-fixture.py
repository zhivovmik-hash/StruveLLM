"""A tool spawns a child then exceeds its timeout; the whole tree must stop."""
import subprocess,sys,pathlib,time
child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(120)'],creationflags=subprocess.CREATE_NO_WINDOW)
pathlib.Path(sys.argv[1]).write_text(str(child.pid),encoding='ascii')
time.sleep(120)
