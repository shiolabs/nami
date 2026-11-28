#!/bin/bash
# Runs inside a privileged Ubuntu container: build VS Code, check it runs, push it.
set -u
nami update --from /i.xz --unsigned >/dev/null
SECONDS=0
nami build /ex/vscode 2>&1 | grep -vE "^  [a-z0-9]{32}-" | tail -6
echo "took ${SECONDS}s"
C=$(ls -d /nix/store/*-vscode-1.141.0 | head -1)
"$C/bin/code" --version --no-sandbox --user-data-dir /tmp/u 2>&1 | head -3
nami keygen test >/dev/null 2>&1
nami push /cache/c /ex/vscode 2>&1 | tail -1
cp /nix/var/nami/keys/test.public /cache/
chown -R "$HOST_UID" /cache
