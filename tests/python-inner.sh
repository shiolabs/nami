#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/python.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -12; fi; }

/src/install.sh /new/nami >/dev/null
nami update --from /i.xz --unsigned >/dev/null 2>&1
nami daemon 2>/tmp/daemon.log &
sleep 1
useradd -m -s /bin/bash alice
as_alice() { su - alice -c "$1"; }
in_app() { as_alice "cd ~/app && $1"; }

as_alice 'mkdir -p ~/app && cd ~/app && cat > project.nami' <<'YAML'
packages:
  - python3:
      version: 3.13
      with: [numpy, requests]
YAML

echo "== in a project"
check "nami run: its Python imports them" "in_app \"nami run -- python3 -c 'import numpy, requests; print(numpy.__version__, requests.__version__)'\" | grep -Eq '^[0-9.]+ [0-9.]+$'"
check "it's Python 3.13, the line asked for" "in_app \"nami run -- python3 -c 'import sys; print(sys.version)'\" | grep -q '^3\.13\.'"
check "a package's own Python dependency is there too (urllib3, for requests)" "in_app \"nami run -- python -c 'import urllib3, idna, certifi'\""
check "its prefix is the env, and ~/.local's packages stay out" "in_app \"nami run -- python3 -c 'import sys, site; print(sys.prefix, site.ENABLE_USER_SITE)'\" | grep -Eq -- '-python3-3\.13\.[0-9]+-env False$'"
check "pip list lists them" "in_app 'nami run -- pip list' > /tmp/pip; grep -Eq '^numpy ' /tmp/pip && grep -Eq '^requests ' /tmp/pip && grep -Eq '^urllib3 ' /tmp/pip"
check "python -m pip too" "in_app 'nami run -- python3 -m pip list' | grep -Eq '^numpy '"
check "a listed package's program is on PATH (numpy's f2py)" "in_app 'nami run -- f2py -v' | grep -Eq '^[0-9]+\.[0-9]+'"
check "and runs with the whole Python: a program of one package imports another" "in_app \"nami run -- sh -c 'command -v normalizer && normalizer --version'\" | grep -qi normalizer"
check "a script with #!/usr/bin/env python3 gets it" "as_alice 'cd ~/app && printf \"#!/usr/bin/env python3\nimport requests\nprint(\\\"ok\\\", requests.__name__)\n\" > s.py && chmod +x s.py && nami run -- ./s.py' | grep -qx 'ok requests'"
check "it starts as fast as Python does (under a second for -c pass, warm)" "in_app 'nami run -- sh -c \"time -p python3 -c pass\"' 2>&1 | awk '/^real/ { exit !(\$2 < 1) }'"
check "nami.lock pins it, with what it's made of" "grep -q '\"python313 with numpy, requests\"' /home/alice/app/nami.lock && grep -q '\"inputs\"' /home/alice/app/nami.lock && grep -q 'python3.13-numpy' /home/alice/app/nami.lock"
check "--locked runs from the lock" "in_app \"nami run --locked -- python3 -c 'import numpy'\""
env_paths=$(python3 -I -c 'import json,sys; print(" ".join(json.load(open(sys.argv[1]))["packages"]["python313 with numpy, requests"]["paths"]))' /home/alice/app/nami.lock 2>/dev/null || grep -o '/nix/store/[a-z0-9]*-python3-3\.13[^"]*-env[^"]*' /home/alice/app/nami.lock | sort -u | tr '\n' ' ')
check "the lock names its two paths" "[ \$(echo $env_paths | wc -w) = 2 ]"
check "nami gc keeps them while the project is there" "nami gc >/dev/null 2>&1 && for p in $env_paths; do test -e \$p || exit 1; done"
# A computer without them (as after a clone): made again, the same, from the lock.
for p in $env_paths; do chmod -R u+w "$p" 2>/dev/null; rm -rf "$p" "/nix/var/nami/db/$(basename "$p").json"; done
check "where they're gone, --locked makes the same two again from what the lock says" "in_app \"nami run --locked -- python3 -c 'import numpy'\" && for p in $env_paths; do test -e \$p || exit 1; done"

echo "== a package its Python's set doesn't have"
as_alice 'mkdir -p ~/typo && cd ~/typo && printf "packages:\n  - python3:\n      version: 3.13\n      with: [numpyy]\n" > project.nami'
check "says which, and what it might be" "as_alice 'cd ~/typo && nami run -- true' 2>&1 | grep -q \"numpyy isn't in python313Packages.*did you mean .numpy.\""
as_alice 'printf "packages:\n  - python3: { with: [requets] }\n" > ~/typo/user.nami'
check "apply --json: a coded error, with what it might be" "as_alice 'nami apply ~/typo/user.nami --json' > /tmp/j 2>&1; grep -Eq '\"code\":\"(invalid_file|not_found)\"' /tmp/j && grep -q 'did you mean .requests.' /tmp/j"
check "nami check says so too, at its line" "as_alice 'nami check ~/typo/project.nami' 2>&1 | grep -q 'project.nami:2.*numpyy'"
as_alice 'mkdir -p ~/old && cd ~/old && printf "packages:\n  - python3:\n      version: 3.9\n      with: [numpy]\n" > project.nami'
check "a Python the catalog has no packages for: which it has" "as_alice 'cd ~/old && nami run -- true' 2>&1 | grep -q 'the catalog has packages for python3'"

echo "== in a person's apps"
as_alice 'cat > ~/user.nami' <<'YAML'
packages:
  - hello
  - python3:
      with: [requests]
YAML
check "nami apply installs it" "as_alice 'nami apply' >/dev/null 2>&1 && as_alice \"python3 -c 'import requests'\""
check "pip is theirs too" "as_alice 'pip list' | grep -Eq '^requests '"
check "nami list names it" "as_alice 'nami list' | grep -q 'python3 with requests'"
check "and --json" "as_alice 'nami list --json' | grep -q '\"attr\":\"python3 with requests\"'"
check "nami info says what it is" "as_alice \"nami info 'python3 with requests'\" | grep -q 'with requests (and pip), from python3Packages'"
as_alice 'printf "packages:\n  - hello\n" > ~/user.nami'
check "taking it off takes it away" "as_alice 'nami apply' >/dev/null 2>&1 && ! as_alice 'command -v python3'"
check "nami undo brings it back" "as_alice 'nami undo' >/dev/null 2>&1 && as_alice \"python3 -c 'import requests'\""

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; tail -5 /tmp/daemon.log; exit 1; }
