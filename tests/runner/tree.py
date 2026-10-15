import subprocess, sys
child = subprocess.Popen([sys.executable, "-u", "-c", "import subprocess,sys,time; subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); print('child-live', flush=True); time.sleep(60)"])
print(child.pid, flush=True)
