"""A dependency-free example that only prints diagnostics."""
import os
import sys
import time

print("Hello from RScriptoBox", flush=True)
print(f"Python: {sys.version.split()[0]}")
print(f"Interpreter: {sys.executable}")
print(f"Working directory: {os.getcwd()}")
print(f"Arguments: {sys.argv[1:]}")
for step in range(1, 4):
    print(f"Progress {step}/3", flush=True)
    time.sleep(0.3)
print("Done. This script did not change your files.")
