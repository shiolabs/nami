#!/bin/sh
# A project's services, for real: postgres and redis from the binary cache,
# started by `nami up -d` for a scratch project, used over their sockets
# (psql, redis-cli), seen by `nami project services` and `nami logs`, then
# stopped by `nami down` with nothing left running; `nami up` in the
# foreground until Ctrl-C; a taken port explained; a service that crashes
# restarted, in the background and the foreground; postgres in a project
# deep in the file system (its socket somewhere short); `nami up` as root
# (postgres runs as nobody then); a slow start and its ready-timeout.
#
# No root, Docker or Nix needed. nami runs as root of a user namespace with
# a scratch store mounted at /nix/store (where the store's programs expect
# it), as `nami daemon`; the project is used by an ordinary user (postgres
# won't run as root) in a user namespace inside that one, whose nami asks
# the daemon for what it needs, like any user's. nami itself runs through a
# copy of its loader and libraries, since the mount hides the host's store.
# usage: tests/project-services.sh   (work files: ${NAMI_SERVICES_DIR:-~/.cache/nami-project-services-test};
#        the store there is kept for the next run)
set -eu
# A test: namios.dev counts its update checks and downloads apart (NAMI_TEST, site/worker.js).
export NAMI_TEST=1
cd "$(dirname "$0")/.."
REPO=$PWD
W=${NAMI_SERVICES_DIR:-$HOME/.cache/nami-project-services-test}
CHANNEL=${NAMI_CHANNEL:-https://namios.dev/menu/nixos-unstable}
KEY=${NAMI_KEY:-tray-2:uHz+KI06voKMb/xTAfd9nd4d8y008M5x5xWFy/pbpIs=}

if [ "${1:-}" != inner ]; then
  cargo build -q
  mkdir -p "$W/root/nix/store" "$W/lib" "$W/home"
  # nami's loader and libraries, outside /nix/store.
  for lib in $(ldd target/debug/nami | grep -o '/nix/store/[^ ]*'); do cp -Lf "$lib" "$W/lib/"; done
  exec unshare -rm sh "$0" inner
fi

# Root of a user namespace, with a mount namespace of our own. The host's
# programs only: its /nix/store is hidden from here on.
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
R=$W/root
mount --bind "$R/nix/store" /nix/store
T=$PWD/target/debug/nami
# nami as root (this namespace's), and as an ordinary user: uid 1000 in a
# namespace inside this one.
nami() { NAMI_ROOT=$R "$W/lib/ld-linux-x86-64.so.2" --library-path "$W/lib" "$T" "$@"; }
U=$W/nami-as-user
cat > "$U" <<EOF
#!/bin/sh
exec unshare --map-user=1000 --map-group=1000 env NAMI_ROOT="$R" USER=dev HOME="$W/home" "$W/lib/ld-linux-x86-64.so.2" --library-path "$W/lib" "$T" "\$@"
EOF
chmod +x "$U"
# And as this namespace's root, as `nami up` in a container or CI may be.
RT=$W/nami-as-root
cat > "$RT" <<EOF
#!/bin/sh
exec env NAMI_ROOT="$R" HOME="$W/root-home" "$W/lib/ld-linux-x86-64.so.2" --library-path "$W/lib" "$T" "\$@"
EOF
chmod +x "$RT"
mkdir -p "$W/root-home"
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if sh -c "$2" >"$W/out" 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' "$W/out" | tail -15; fi; }

[ -f "$R/nix/var/nami/index.json" ] || nami update --from "$CHANNEL" --trust "$KEY" >/dev/null
rm -f "$R/nix/var/nami/daemon.sock"
NAMI_ROOT=$R "$W/lib/ld-linux-x86-64.so.2" --library-path "$W/lib" "$T" daemon >"$W/daemon.log" 2>&1 &
daemon=$!
trap 'kill $daemon 2>/dev/null' EXIT
timeout 20 sh -c "until [ -S '$R/nix/var/nami/daemon.sock' ]; do sleep 0.2; done"

P=$W/shop
rm -rf "${W:?}/shop" "${W:?}/other" "${W:?}/crash" "${W:?}/deep" "${W:?}/asroot" "${W:?}/slow"
mkdir -p "$P" "$W/other"
# Each service's supervisor, and the service's process group.
procs() { awk '{ print $1, $4 }' .nami/*.pid | tr "\n" " "; }
# Ports of their own: the host may have a postgres or redis of its own.
# Their files copied in from nami's examples, as a project of yours does.
copy_services() { mkdir -p "$1/services" && cp examples/services/postgres.nami examples/services/redis.nami "$1/services/"; }
(cd "$REPO" && copy_services "$P")
cat > "$P/project.nami" <<'EOF'
services:
  - ./services/postgres.nami: { port: 15432 }
  - ./services/redis.nami: { port: 16379 }
tasks:
  one: psql -tAc 'select 1'
EOF
cd "$P"

echo "== the environment, before anything runs"
"$U" env > "$W/env"
check "nami env has DATABASE_URL, PGHOST, REDIS_URL" "grep -qx \"export DATABASE_URL='postgresql://postgres@localhost:15432/shop'\" '$W/env' && grep -qx \"export PGHOST='$P/.nami/postgres'\" '$W/env' && grep -qx \"export REDIS_URL='redis://localhost:16379'\" '$W/env'"
check "their packages are pinned in nami.lock" "grep -q '\"postgresql\"' nami.lock && grep -q '\"redis\"' nami.lock"
check "nami project services: stopped" "$U project services | grep -q '^postgres  stopped'"

echo "== nami up -d"
check "starts them, back once they're ready" "$U up -d && $U project services | grep -q '^redis     running'"
check "psql over the socket" "$U run -- psql -tAc 'select 1' | grep -qx 1"
check "a task with them" "$U run one | grep -qx 1"
check "redis-cli ping over the socket" "$U run -- sh -c 'redis-cli -s \"\$REDIS_SOCKET\" ping' | grep -qx PONG"
check "redis-cli over TCP" "$U run -- redis-cli -p 16379 ping | grep -qx PONG"
kill -KILL "$(cut -d' ' -f4 .nami/redis.pid)"
check "redis killed: it's restarted, and answers again" "timeout 20 sh -c 'until grep -q \"restarting in 1s\" .nami/redis.log && $U run -- redis-cli -p 16379 ping 2>/dev/null | grep -qx PONG; do sleep 0.3; done' && $U project services | grep -q '^redis     running .*(restarted once)'"
check "data, sockets, logs in .nami, kept out of git" "[ -S .nami/redis/redis.sock ] && [ -S .nami/postgres/.s.PGSQL.15432 ] && [ -f .nami/postgres/data/PG_VERSION ] && [ -s .nami/postgres.log ] && grep -qx '[*]' .nami/.gitignore"
check "a second nami up -d says they're running" "$U up -d 2>&1 | grep -q 'already running: postgres, redis'"
check "nami up says so instead of starting twice" "! $U up >'$W/o' 2>&1 && grep -q 'already running in the background: postgres, redis' '$W/o'"
check "nami logs postgres" "timeout 3 $U logs postgres | grep -q 'ready to accept connections'"
"$U" run -- psql -qc 'create table kept (x int); insert into kept values (42)'
pgids=$(procs)
echo "== nami down"
check "stops them" "$U down 2>&1 | grep -q 'postgres: stopped'"
check "nothing left running, nothing in their process groups" "for g in $pgids; do ps -eo pgid= | grep -qw \$g && exit 1; done; ! ls .nami/*.pid"
check "nami down again: nothing is running" "$U down 2>&1 | grep -q 'nothing is running'"

echo "== nami up in the foreground, until Ctrl-C"
"$U" up >"$W/fg.log" 2>&1 &
fg=$!
timeout 90 sh -c "until grep -q '^redis *| ready' '$W/fg.log'; do sleep 0.5; done" || true
check "output prefixed with their names" "grep -q '^postgres | .*ready to accept connections' '$W/fg.log' && grep -q '^redis    | ' '$W/fg.log'"
check "the data is still there" "$U run -- psql -tAc 'select x from kept' | grep -qx 42"
fgpgids=$(procs)
kill -INT $fg
check "Ctrl-C stops them all, cleanly" "timeout 30 sh -c 'while kill -0 $fg 2>/dev/null; do sleep 0.2; done' && grep -q '^postgres | stopped' '$W/fg.log' && grep -q '^redis    | stopped' '$W/fg.log'"
check "and leaves nothing running" "for g in $fgpgids; do ps -eo pgid= | grep -qw \$g && exit 1; done; ! ls .nami/*.pid"

echo "== a taken port"
cat > "$W/other/project.nami" <<'EOF'
services:
  - web: { run: sleep 600, port: 15433 }
EOF
python3 -I -c 'import socket,time; s=socket.socket(); s.bind(("127.0.0.1",15433)); s.listen(); time.sleep(30)' &
taken=$!
timeout 10 sh -c "until python3 -I -c 'import socket; socket.create_connection((\"127.0.0.1\",15433))' 2>/dev/null; do sleep 0.2; done"
check "says which service, which port, how to change it" "! $U up -d --project '$W/other' >'$W/o' 2>&1 && cat '$W/o' && grep -q 'web can.t start: port 15433 is already in use' '$W/o' && grep -q 'web: { port: 15434 }' '$W/o'"
kill $taken 2>/dev/null || true

echo "== a service that crashes is restarted"
C=$W/crash
mkdir -p "$C"
cat > "$C/project.nami" <<'EOF'
services:
  - crashy: { run: "echo up; sleep 1; exit 3" }
  - once: { run: "sleep 1; exit 3", restart: no }
EOF
cd "$C"
check "nami up -d" "$U up -d"
check "restarted, after 1s, then 2s; nami logs shows it" "timeout 20 sh -c 'until grep -q \"restarting in 2s\" .nami/crashy.log; do sleep 0.3; done' && timeout 3 $U logs crashy | grep -q 'ended (exit status: 3); restarting in 1s'"
check "nami project services: its state, how often it was restarted" "$U project services | grep -Eq '^crashy +(running|restarting|starting) .*restarted [0-9]+ times'"
check "restart: no: it ends, and it failed" "timeout 10 sh -c 'until $U project services | grep -Eq \"^once +failed\"; do sleep 0.3; done'"
cgroups=$(procs)
check "nami down stops it (between restarts too), nothing left" "$U down && for g in $cgroups; do ps -eo pgid= | grep -qw \$g && exit 1; done; ! ls .nami/*.pid"
"$U" up >"$W/fg2.log" 2>&1 &
fg=$!
timeout 20 sh -c "until grep -q '^crashy | ended (exit status: 3); restarting in 1s' '$W/fg2.log'; do sleep 0.3; done" || true
check "in the foreground too" "grep -q '^crashy | ended (exit status: 3); restarting in 1s' '$W/fg2.log' && grep -q '^crashy | up' '$W/fg2.log'"
kill -INT $fg
check "and Ctrl-C stops it" "timeout 30 sh -c 'while kill -0 $fg 2>/dev/null; do sleep 0.2; done' && grep -q '^crashy | stopped' '$W/fg2.log' && ! ls .nami/*.pid"

echo "== postgres in a project deep in the file system"
D=$W/deep/a-folder-with-a-rather-long-name/and-another-one-inside-it/the-project
mkdir -p "$D"
(cd "$REPO" && copy_services "$D")
cat > "$D/project.nami" <<'EOF'
services:
  - ./services/postgres.nami: { port: 15434 }
EOF
cd "$D"
check "its socket goes somewhere short, and PGHOST says where" "$U env | grep '^export PGHOST=' | grep -v '$D' | grep -q /nami"
check "nami up -d, psql over the socket" "$U up -d && $U run -- psql -tAc 'select 1' | grep -qx 1"
check "nami down" "$U down 2>&1 | grep -q 'postgres: stopped' && ! ls .nami/*.pid"

echo "== nami up as root: postgres runs as nobody"
A=$W/asroot
mkdir -p "$A"
(cd "$REPO" && copy_services "$A")
cat > "$A/project.nami" <<'EOF'
services:
  - ./services/postgres.nami: { port: 15435 }
EOF
cd "$A"
check "nami up -d as root" "$RT up -d"
check "psql (as root) connects" "$RT run -- psql -tAc 'select 1' | grep -qx 1"
check "postgres runs as nobody, its data stays root's" "[ \"\$(stat -c %u .nami/postgres/data)\" = 0 ] && grep -q 'ready to accept connections' .nami/postgres.log"
check "nami down as root" "$RT down 2>&1 | grep -q 'postgres: stopped' && ! ls .nami/*.pid"

echo "== a slow start"
S=$W/slow
mkdir -p "$S"
cat > "$S/project.nami" <<'EOF'
services:
  - slow: { run: "sleep 4; touch {state}/up; exec sleep 600", ready: "test -f {state}/up", ready-timeout: 1 }
EOF
cd "$S"
check "not ready by its ready-timeout: says how to wait longer, stops it" "! $U up -d >'$W/o' 2>&1 && grep -q \"slow isn't ready after 1s\" '$W/o' && grep -q 'ready-timeout: 4' '$W/o' && ! ls .nami/*.pid"
sed -i 's/ready-timeout: 1/ready-timeout: 30/' project.nami
check "with a longer one, it waits" "$U up -d && $U project services | grep -q '^slow  running'"
check "nami down" "$U down 2>&1 | grep -q 'slow: stopped'"

cd /
rm -rf "${W:?}/shop" "${W:?}/other" "${W:?}/crash" "${W:?}/deep" "${W:?}/asroot" "${W:?}/slow"
echo
[ $fails -eq 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
