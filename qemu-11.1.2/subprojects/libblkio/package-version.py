#!/usr/bin/env python3
import json
import subprocess

out = subprocess.check_output(["cargo", "read-manifest"])
print(json.loads(out)["version"])
