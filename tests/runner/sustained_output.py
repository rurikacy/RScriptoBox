"""Emit 60 MiB over 60 seconds, alternating flushed stdout/stderr blocks."""
import sys
import time

block = b"x" * (16 * 1024)
started = time.monotonic()
for index in range(60 * 64):
    stream = sys.stdout.buffer if index % 2 == 0 else sys.stderr.buffer
    stream.write(block)
    stream.flush()
    time.sleep(max(0, started + (index + 1) / 64 - time.monotonic()))
