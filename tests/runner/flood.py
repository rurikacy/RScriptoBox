import os, threading
def write(fd):
    for _ in range(256):
        os.write(fd, b"x" * 8192)
threads = [threading.Thread(target=write, args=(fd,)) for fd in (1, 2)]
for thread in threads: thread.start()
for thread in threads: thread.join()
