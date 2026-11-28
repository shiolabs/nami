#!/bin/sh
# Runs inside the nixos/nix container, or directly on a machine with Nix;
# see build-index.sh.
# usage: [OUT=dir (default /out)] [WORKERS=n] [ONLY="rPackages emacsPackages"] [SYSTEM=aarch64-linux] in-container.sh <release-url>
#   ONLY: just these top-level names (index/catalog.nix), to try a few sets.
#   SYSTEM: another platform's index, index-<system>.json (ARM Linux's, aarch64-linux,
#   and a Mac's: see below). Its packages' names, store paths, descriptions and main
#   programs, and which the cache hasn't got; no definitions, no closures. An x86_64
#   nami's `nami bundle --arch aarch64` reads ARM Linux's too.
set -eu
release=$1
OUT=${OUT:-/out}
CATALOG=${CATALOG:-/catalog.nix}
CLOSURES=${CLOSURES:-/closures.py}
UNCACHED=${UNCACHED:-/uncached.py}
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
# Evaluation workers restart past 2 GB but can briefly use more; leave room.
# (nixos/nix has no awk or sed.)
mem_kb=0; while read -r k v _; do [ "$k" != MemAvailable: ] || mem_kb=$v; done < /proc/meminfo
mem_gb=$((mem_kb / 1048576))
WORKERS=${WORKERS:-$(( mem_gb / 3 < $(nproc) ? (mem_gb / 3 > 0 ? mem_gb / 3 : 1) : $(nproc) ))}
export NIX_CONFIG='experimental-features = nix-command flakes'

curl -sfL "$release/git-revision" > $OUT/git-revision
# The container has only Nix: its tools come from nixpkgs. (With nami or
# nix develop they're on PATH already.)
command -v nix-eval-jobs >/dev/null && command -v jq >/dev/null && command -v zstd >/dev/null ||
for p in $(nix build --no-link --print-out-paths nixpkgs#nix-eval-jobs nixpkgs#jq.bin nixpkgs#zstd.bin 2>/dev/null); do
  PATH=$p/bin:$PATH
done

# One JSON object per package: {attr, name, drvPath, outputs: {out: /nix/store/...}}.
# What's in it: index/catalog.nix. Workers restart past --max-memory-size,
# so this fits in a few GB of RAM per worker.
# An existing $OUT/jobs.jsonl (from an interrupted run) is reused.
only=null
[ -z "${ONLY:-}" ] || only="[ $(printf '"%s" ' $ONLY)]"
if [ -s $OUT/jobs.jsonl ]; then cp $OUT/jobs.jsonl $T/jobs.jsonl; else
nix-eval-jobs --workers "$WORKERS" --max-memory-size 2048 --meta \
  --expr "import $CATALOG { nixpkgs = builtins.fetchTarball \"$release/nixexprs.tar.xz\"; only = $only; system = \"${SYSTEM:-x86_64-linux}\"; }" \
  2>$OUT/eval-errors.log > $T/jobs.jsonl
cp $T/jobs.jsonl $OUT/jobs.jsonl   # raw evaluation, kept to rerun later steps without re-evaluating
fi

# A Mac's (aarch64-darwin, x86_64-darwin) and ARM Linux's (aarch64-linux): a catalog of
# its own for nami there. What search and info show (description, homepage, license, main
# program), and which packages cache.nixos.org has no build of (`uncached`: nami says they
# aren't ready-built for Mac, or for ARM Linux). No build definitions or closures yet:
# nothing of the catalog's is built on a Mac, nor on ARM Linux for now (docs/arm.md).
case ${SYSTEM:-x86_64-linux} in *-darwin|aarch64-linux)
  PY=$(command -v python3 || echo "$(nix build --no-link --print-out-paths nixpkgs#python3 2>/dev/null)/bin/python3")
  "$PY" "$UNCACHED" find-all $T/jobs.jsonl $OUT
  jq -n --rawfile uncached $OUT/uncached.txt '
    ($uncached | split("\n") | map(select(length > 0) | {key: ., value: true}) | from_entries) as $nocache
    | reduce (inputs | select(.outputs)) as $j ({};
        .[$j.attr] = ({name: $j.name, outputs: $j.outputs}
          + (if $j.meta.description then {description: $j.meta.description} else {} end)
          + (if ($j.meta.homepage | type) == "string" then {homepage: $j.meta.homepage} else {} end)
          + (if $j.meta.mainProgram then {mainProgram: $j.meta.mainProgram} else {} end)
          + (($j.meta.license | if type == "array" then .[0] else . end) as $l
             | if ($l | type) == "object" and $l.spdxId then {license: $l.spdxId} else {} end)
          + (if $j.meta.unfree == true then {unfree: true} else {} end)
          + (if $nocache[$j.attr] then {uncached: true} else {} end)))' \
    $T/jobs.jsonl > $OUT/index-$SYSTEM.json
  echo "index-$SYSTEM.json: $(jq length $OUT/index-$SYSTEM.json) packages, $(grep -c . $OUT/uncached.txt || true) not ready-built"
  exit 0
esac

# Another platform's: its packages' names and store paths, and that's all.
if [ "${SYSTEM:-x86_64-linux}" != x86_64-linux ]; then
  jq -n 'reduce (inputs | select(.outputs)) as $j ({};
      .[$j.attr] = ({name: $j.name, outputs: $j.outputs}
        + (if $j.meta.mainProgram then {mainProgram: $j.meta.mainProgram} else {} end)))' \
    $T/jobs.jsonl > $OUT/index-$SYSTEM.json
  echo "index-$SYSTEM.json: $(jq length $OUT/index-$SYSTEM.json) packages"
  exit 0
fi

# Build definitions, one file per attr, for recipes that `extends:` a nixpkgs
# package. .drv files aren't in cache.nixos.org, so the index ships them.
jq -s 'reduce (.[] | select(.drvPath)) as $j ({}; .[$j.drvPath | ltrimstr("/nix/store/")] += [$j.attr])' \
  $T/jobs.jsonl > $T/drv-attrs.json
mkdir -p $OUT/drvs
[ -n "$(ls $OUT/drvs)" ] || jq -r 'keys[] | "/nix/store/" + .' $T/drv-attrs.json \
  | xargs -n 500 nix derivation show 2>/dev/null \
  | jq -r --slurpfile m $T/drv-attrs.json \
      '.derivations | to_entries[] | . as $e | ($m[0][$e.key] // [])[] | "\(.)\t\({($e.key): $e.value} | tojson)"' \
  | while IFS="$(printf '\t')" read -r attr json; do
      printf '%s\n' "$json" > "$OUT/drvs/$attr.json"
    done
echo "exported $(ls $OUT/drvs | wc -l) derivations"

# Unfree packages: Hydra doesn't build them, so nami builds them on the
# user's machine from a closure of build definitions (index/closures.py).
# The kernels' unfree modules are left out, but for NVIDIA's driver for the
# system's kernel (packages/nvidia/package.nami): its libraries are the same
# for every kernel, and its kernel modules are free (open). (CUDA's take
# seconds: NVIDIA's own downloads, unpacked.)
PY=$(command -v python3 || echo "$(nix build --no-link --print-out-paths nixpkgs#python3 2>/dev/null)/bin/python3")
jq -r 'select(.drvPath and .meta.unfree == true) | select((.attr | test("^(linuxKernel|linuxPackages|tests\\.)") | not) or (.attr | test("^linuxPackages\\.nvidia_x11(_[a-z0-9_]+)?$"))) | [.attr, .drvPath] | @tsv' \
  $T/jobs.jsonl > $T/unfree.tsv
echo "$(wc -l < $T/unfree.tsv) unfree packages"
"$PY" "$CLOSURES" $T/unfree.tsv $OUT > $OUT/closures.log 2>&1 || true
# Free packages the binary cache doesn't have (index/uncached.py): marked,
# and the ones nami can build (they need software that isn't free, or
# Hydra doesn't build them) get a closure too. Hydra doesn't build the sets
# index/catalog.nix adds (but for the few hundred of their most used): their
# packages' closures come in parts, shared (closures.py --parts).
unbuilt="rPackages emacsPackages coqPackages_8_20 cmdpack gerbilPackages-unstable"
"$PY" "$UNCACHED" find $T/jobs.jsonl $OUT $unbuilt > $T/uncached.tsv
"$PY" "$CLOSURES" --parts $T/uncached.tsv $OUT >> $OUT/closures.log 2>&1 || true
"$PY" "$UNCACHED" keep $T/jobs.jsonl $OUT $T/unfree.tsv $unbuilt
echo "$(ls $OUT/drvs | grep -c '\.closure\.json$') build closures"
# Each definition's sha256 goes into the index, so one signature over the
# index (nami catalog sign) covers every definition too.
# (Attr names may contain quotes, e.g. pkg."dbus-1", so names go NUL-separated.)
(cd $OUT/drvs && find . -name '*.json' -printf '%P\0' | xargs -0 -n 2000 sha256sum) > $T/drv-hashes.txt

# nami index format: { attr: {name, outputs, description, homepage, mainProgram, license, drv} };
# index/apps.py adds apps and icons afterwards.
jq -n --rawfile drvs $T/drv-hashes.txt --rawfile uncached $OUT/uncached.txt '
  ($drvs | split("\n") | map(select(length > 0) | split("  ") | {key: (.[1] | rtrimstr(".json")), value: .[0]}) | from_entries) as $drv
  | ($uncached | split("\n") | map(select(length > 0) | {key: ., value: true}) | from_entries) as $nocache
  | reduce (inputs | select(.outputs)) as $j ({};
      .[$j.attr] = ({name: $j.name, outputs: $j.outputs}
        + (if $j.meta.description then {description: $j.meta.description} else {} end)
        + (if ($j.meta.homepage | type) == "string" then {homepage: $j.meta.homepage} else {} end)
        + (if $j.meta.mainProgram then {mainProgram: $j.meta.mainProgram} else {} end)
        + (($j.meta.license | if type == "array" then .[0] else . end) as $l
           | if ($l | type) == "object" and $l.spdxId then {license: $l.spdxId} else {} end)
        + (if $j.meta.unfree == true then {unfree: true} else {} end)
        + (if $nocache[$j.attr] then {uncached: true} else {} end)
        + (if $drv[$j.attr] then {drv: $drv[$j.attr]} else {} end)
        + (if $drv[$j.attr + ".closure"] then {closure: $drv[$j.attr + ".closure"]} else {} end)))' \
  $T/jobs.jsonl > $OUT/index-x86_64-linux.json


