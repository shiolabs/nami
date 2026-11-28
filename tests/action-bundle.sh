#!/usr/bin/env sh
# nami-bundle-action (action/), as GitHub's runner would run it, in a clean
# Ubuntu container: a user "runner" with sudo and no nami, a checkout of a
# small Rust project tagged v0.3.1 as the workspace, GitHub's variables
# (GITHUB_REF, GITHUB_OUTPUT, RUNNER_TEMP, ...), the inputs as action.yml
# passes them, and a stand-in `gh` that keeps what it's asked to upload.
#
#   - action.yml passes every input to bundle.sh
#   - nami comes from its installer (the published release), then this
#     nami over it (the nami-binary input), and its daemon is started
#   - nami bundle . makes the bundle, its AppImage and a thin one, each
#     with a .sha256 that checks; the release is made and they're uploaded
#   - the step's outputs and summary; the bundle runs
#   - on a branch (no tag), nothing is uploaded
#
# usage: tests/action-bundle.sh   NAMI=target/release/nami (static, ./build.sh)  OUT=target/action-bundle
set -u
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
nami=$(realpath "${NAMI:-target/release/nami}")
mkdir -p "${OUT:-target/action-bundle}"
out=$(realpath "${OUT:-target/action-bundle}")
rm -rf "$out/release" "$out/gh.log"
fail=0

# Every input action.yml has reaches bundle.sh, as INPUT_<NAME>.
for input in $(sed -n '/^inputs:/,/^outputs:/p' action/action.yml | grep -E '^  [a-z-]+:$' | tr -d ' :'); do
  var=INPUT_$(echo "$input" | tr 'a-z-' 'A-Z_')
  [ "$input" = token ] && var=GH_TOKEN
  if ! grep -q "        $var: \${{ inputs.$input }}" action/action.yml || ! grep -q "$var" action/bundle.sh; then
    echo "  FAIL action.yml's input $input doesn't reach bundle.sh as $var"; fail=$((fail + 1))
  fi
done
[ "$fail" -eq 0 ] && echo "  ok   action.yml passes each input to bundle.sh"

# The installer the action fetches, from nami's site. It checks the release
# against the keys it knows with openssl, where there is one. Where the
# release is signed with a key it doesn't know (the installer not redeployed
# after a new key), that's said, and the runner's openssl is hidden while it
# installs (as on a computer without one), so the rest is still tested.
base=https://namios.dev
curl -fsSL "$base/install?via=test" -o "$out/install.sh" || { echo "  FAIL fetching $base/install"; exit 1; }
signed_by=$(curl -fsSL "$base/releases/$(curl -fsSL "$base/releases/latest?via=test")/SHA256SUMS.sig" | cut -d: -f1)
# (An installer from before packaging/keys knows just its KEY.)
trusts=$(sed -n 's/^KEYS="\(.*\)"/\1/p' "$out/install.sh")
[ -n "$trusts" ] || trusts=$(sed -n 's/^KEY="\([^"]*\)".*/\1/p' "$out/install.sh")
trusts=$(for k in $trusts; do printf '%s ' "${k%%:*}"; done | sed 's/ $//')
hide_openssl=
case " $trusts " in
  *" $signed_by "*) echo "  ok   the site's installer knows the key its release is signed with ($signed_by)" ;;
  *)
    echo "  NOTE the site's installer knows $trusts, but its release is signed by $signed_by: where there's"
    echo "       openssl 3 (GitHub's runners) it refuses to install. Redeploy the installer. Testing without openssl."
    hide_openssl=1 ;;
esac

docker run --rm -e HIDE_OPENSSL=$hide_openssl -v "$nami":/opt/nami-under-test:ro -v "$PWD/action":/action:ro -v "$out/install.sh":/installer/install.sh:ro \
  -v "$PWD/tests/fixtures/bundle-projects/rust-hello":/src:ro -v "$out":/out ubuntu:24.04 bash -c '
  set -u
  fails=0
  ok() { echo "  ok   $1"; }
  bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
  check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; tail -20 /tmp/out | sed "s/^/       /"; fi; }
  # What GitHub'"'"'s Ubuntu runner has: curl, sudo, git, a user with sudo.
  apt-get update -qq >/dev/null && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends curl ca-certificates sudo git procps >/dev/null
  useradd -m runner && echo "runner ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/runner && mkdir -m 777 /gh
  # gh: "release view" finds no release, the rest is kept in /out.
  cat > /usr/local/bin/gh <<"GH"
#!/bin/sh
echo "gh $*" >> /gh/gh.log
[ "$GH_TOKEN" = test-token ] || { echo "no token" >&2; exit 4; }
case "$1 $2" in
  "release view") exit 1 ;;
  "release create") exit 0 ;;
  "release upload") shift 3; mkdir -p /gh/release; for f; do case $f in --*|acme/*) ;; *) cp "$f" /gh/release/ ;; esac; done ;;
esac
GH
  chmod +x /usr/local/bin/gh
  W=/home/runner/work/rust-hello/rust-hello
  mkdir -p $W /home/runner/work/_temp && cp -r /src/. $W && chown -R runner /home/runner
  sudo -u runner sh -c "cd $W && git init -q && git add -A && git -c user.name=ci -c user.email=ci@example.com commit -qm v0.3.1 && git tag v0.3.1"
  run_action() { # <ref> <inputs as NAME=value...>
    ref=$1; shift
    : > /tmp/env
    for kv in GITHUB_ACTIONS=true CI=true GITHUB_WORKSPACE=$W GITHUB_REF=$ref GITHUB_REF_NAME=${ref##*/} GITHUB_REPOSITORY=acme/rust-hello \
      GITHUB_ACTION_PATH=/action RUNNER_TEMP=/home/runner/work/_temp GITHUB_OUTPUT=/tmp/gh-output GITHUB_STEP_SUMMARY=/tmp/gh-summary GITHUB_PATH=/tmp/gh-path \
      GH_TOKEN=test-token INPUT_PATH=. INPUT_WHAT= INPUT_ARGS= INPUT_APPIMAGE=false INPUT_THIN=false INPUT_UPLOAD=auto INPUT_TAG= \
      INPUT_INSTALL_URL=file:///installer/install.sh INPUT_NAMI_BINARY=/opt/nami-under-test "$@"; do
      echo "export ${kv%%=*}=\"${kv#*=}\"" >> /tmp/env
    done
    : > /tmp/gh-output; : > /tmp/gh-summary; chmod 666 /tmp/gh-output /tmp/gh-summary /tmp/env; touch /tmp/gh-path; chmod 666 /tmp/gh-path
    # As the step runs it: bash, in the workspace, as the runner.
    sudo -u runner bash -c ". /tmp/env; cd \$GITHUB_WORKSPACE && \"\$GITHUB_ACTION_PATH/bundle.sh\""
  }
  [ -z "${HIDE_OPENSSL:-}" ] || mv /usr/bin/openssl /usr/bin/openssl.hidden
  s=$(date +%s)
  run_action refs/tags/v0.3.1 INPUT_APPIMAGE=true INPUT_THIN=true >/out/run1.log 2>&1 && ok "the action ran, for the tag v0.3.1 ($(( $(date +%s) - s ))s)" || { bad "the action ran, for the tag v0.3.1"; tail -40 /out/run1.log | sed "s/^/       /"; }
  [ -z "${HIDE_OPENSSL:-}" ] || mv /usr/bin/openssl.hidden /usr/bin/openssl
  check "nami came from its installer" "grep -q \"Nami .* is installed\" /out/run1.log"
  check "then the nami under test over it" "grep -q \"using the nami binary /opt/nami-under-test\" /out/run1.log"
  check "its daemon started (no systemd here)" "grep -q \"started nami.s daemon\" /out/run1.log"
  check "nami bundle . found the project" "grep -q \"project: Rust (Cargo.toml)\" /out/run1.log"
  check "it made the release, which there was none of" "grep -q \"^gh release create v0.3.1 --repo acme/rust-hello --verify-tag\" /gh/gh.log"
  check "it uploaded to it" "grep -q \"^gh release upload v0.3.1 --repo acme/rust-hello --clobber\" /gh/gh.log"
  for f in rust-hello-x86_64 rust-hello-x86_64.AppImage rust-hello-thin-x86_64; do
    check "uploaded: $f, and its .sha256, which checks" "test -s /gh/release/$f && cd /gh/release && sha256sum -c $f.sha256"
  done
  check "the outputs: bundle, dir, files" "grep -q \"^bundle=/home/runner/work/_temp/nami-bundle/rust-hello-x86_64$\" /tmp/gh-output && grep -q \"^dir=\" /tmp/gh-output && test \$(sed -n \"/^files<</,/^NAMI_EOF/p\" /tmp/gh-output | grep -c /) -eq 6"
  check "the summary lists them" "grep -q \"rust-hello-x86_64\\.AppImage\" /tmp/gh-summary && grep -q \"Uploaded to the release\" /tmp/gh-summary"
  check "nami is on PATH for later steps" "grep -qx /nix/var/nami/bin /tmp/gh-path"
  check "the bundle runs (in a clean place)" "cp /gh/release/rust-hello-x86_64 /tmp/r && sudo -u nobody env HOME=/tmp/nobody /tmp/r ci | grep -qx \"rust-hello 0.3.1: hello, ci\""
  check "its .sha256 is sha256sum'"'"'s" "grep -Eq \"^[0-9a-f]{64}  rust-hello-x86_64$\" /gh/release/rust-hello-x86_64.sha256"
  # A push to a branch: bundled, not uploaded.
  : > /gh/gh.log
  run_action refs/heads/main >/out/run2.log 2>&1 && ok "the action ran, for a branch" || { bad "the action ran, for a branch"; tail -30 /out/run2.log | sed "s/^/       /"; }
  check "and uploaded nothing, saying why" "grep -q \"not uploading: this run isn.t for a release or a tag\" /out/run2.log && ! grep -q upload /gh/gh.log"
  cp -r /gh/. /out/ 2>/dev/null   # what was uploaded, to look at
  [ "$fails" -eq 0 ]' || fail=$((fail + 1))
[ "$fail" -eq 0 ] && echo "PASS" || { echo "$fail FAILED"; exit 1; }
