import json, os, sys
print(json.dumps({"args": sys.argv[1:], "cwd": os.getcwd(), "env": os.environ.get("RSCRIPTO_TEST")}, ensure_ascii=False), flush=True)
sys.stderr.write("stderr 可见\n")
sys.stderr.flush()
