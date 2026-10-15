"""Build an offline wheel and hostile venv pip.ini for the Rust integration test."""
import pathlib
import sys
import zipfile

project = pathlib.Path(sys.argv[1])
environment = pathlib.Path(sys.argv[2])
project.mkdir(parents=True, exist_ok=True)
wheel = project / "rscripto_probe-0.0.0-py3-none-any.whl"
with zipfile.ZipFile(wheel, "w") as archive:
    archive.writestr("rscripto_probe.py", "value = 'installed-in-selected-venv'\n")
    archive.writestr("rscripto_probe-0.0.0.dist-info/METADATA", "Metadata-Version: 2.1\nName: rscripto-probe\nVersion: 0.0.0\n")
    archive.writestr("rscripto_probe-0.0.0.dist-info/WHEEL", "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n")
    archive.writestr("rscripto_probe-0.0.0.dist-info/RECORD", "")
(project / "requirements.txt").write_text("./" + wheel.name + "\n", encoding="utf-8")
(environment / "pip.ini").write_text("[global]\ntarget = " + str(project / "poisoned-target") + "\nuser = true\nprefix = " + str(project / "poisoned-prefix") + "\n", encoding="utf-8")
