#!/usr/bin/env bash
# nami-bundle-action's work (action.yml runs it): install nami, bundle the
# project, write .sha256 files, upload them to the release.
#
# Everything comes in as INPUT_* variables, and GitHub's own (GITHUB_*,
# RUNNER_TEMP), so it runs the same outside Actions:
#   INPUT_PATH=. GITHUB_WORKSPACE=$PWD action/bundle.sh
# (tests/action-bundle.sh does, in a clean Ubuntu container).
set -euo pipefail

say() { printf '%s\n' "$*"; }
fail() { printf '::error::%s\n' "$*"; exit 1; }
group() { printf '::group::%s\n' "$*"; }
endgroup() { printf '::endgroup::\n'; }
yes() { case ${1,,} in true|yes|1|on) return 0 ;; *) return 1 ;; esac; }

workspace=${GITHUB_WORKSPACE:-$PWD}
project=${INPUT_PATH:-.}
case $project in /*) ;; *) project=$workspace/$project ;; esac
[ -d "$project" ] || fail "path: $INPUT_PATH isn't a folder in the repo"
project=$(cd "$project" && pwd)
out=${RUNNER_TEMP:-/tmp}/nami-bundle
rm -rf "$out" && mkdir -p "$out"

[ "$(uname -m)" = x86_64 ] || fail "nami bundle makes x86_64 Linux bundles, on an x86_64 Linux runner (this is $(uname -sm)): runs-on: ubuntu-latest"
if [ "$(id -u)" -eq 0 ]; then sudo=""; elif command -v sudo >/dev/null; then sudo=sudo; else sudo=""; fi

# 1. nami, from its installer (unless the runner has it already).
group "Installing nami"
if ! command -v nami >/dev/null && [ ! -x /nix/var/nami/bin/nami ]; then
  curl -fsSL "${INPUT_INSTALL_URL:-https://namios.dev/install}" | sh
fi
export PATH="/nix/var/nami/bin:$PATH"
[ -z "${GITHUB_PATH:-}" ] || echo /nix/var/nami/bin >> "$GITHUB_PATH"
if [ -n "${INPUT_NAMI_BINARY:-}" ]; then
  say "using the nami binary $INPUT_NAMI_BINARY"
  $sudo install -m755 "$INPUT_NAMI_BINARY" /nix/var/nami/bin/nami
  ! command -v systemctl >/dev/null || $sudo systemctl try-restart nami-daemon.service 2>/dev/null || true
fi
nami --version
# Not root: the store goes through nami's daemon. The installer starts it
# with systemd; where there's none (a container job), it's started here.
if [ "$(id -u)" -ne 0 ] && ! { command -v systemctl >/dev/null && systemctl is-active -q nami-daemon.service 2>/dev/null; }; then
  if ! pgrep -f 'nami daemon' >/dev/null 2>&1; then
    [ -n "$sudo" ] || fail "nami's daemon isn't running, and there's no sudo to start it"
    $sudo sh -c 'nohup /nix/var/nami/bin/nami daemon >/tmp/nami-daemon.log 2>&1 &'
    for _ in $(seq 50); do [ -S /nix/var/nami/daemon.sock ] && break; sleep 0.2; done
    [ -S /nix/var/nami/daemon.sock ] || { cat /tmp/nami-daemon.log; fail "nami's daemon didn't start"; }
    say "started nami's daemon"
  fi
fi
endgroup

# 2. The bundle (and its variants), named after the program that runs.
what=${INPUT_WHAT:-}
if [ -z "$what" ]; then
  what=$project
elif [ -e "$project/$what" ]; then
  what=$(cd "$(dirname "$project/$what")" && pwd)/$(basename "$what")
fi
extra=()
[ -z "${INPUT_ARGS:-}" ] || eval "extra=($INPUT_ARGS)"
files=()
bundle_one() { # <label> <nami bundle options...>
  local label=$1; shift
  group "nami bundle${label:+ ($label)}"
  (cd "$out" && nami bundle "$what" "${extra[@]}" "$@") > "$out/.path" || { endgroup; fail "nami bundle failed (above)"; }
  endgroup
  made=$(tail -1 "$out/.path"); rm -f "$out/.path"
}
appimage=()
! yes "${INPUT_APPIMAGE:-false}" || appimage=(--appimage)
bundle_one "" "${appimage[@]}"; bundle=$made
case $bundle in /*) ;; *) bundle=$out/$bundle ;; esac
[ -f "$bundle" ] || fail "nami bundle said it wrote $bundle, which isn't there"
name=$(basename "$bundle")
files+=("$name")
[ ${#appimage[@]} -eq 0 ] || { [ -f "$bundle.AppImage" ] && files+=("$name.AppImage"); }
if yes "${INPUT_THIN:-false}"; then
  thin=${name%-x86_64}-thin-x86_64
  bundle_one thin --thin -o "$thin"
  files+=("$thin")
fi

# 3. A .sha256 for each, as sha256sum -c reads it.
summary="| File | Size | sha256 |\n|---|---|---|\n"
for f in "${files[@]}"; do
  (cd "$out" && sha256sum "$f" > "$f.sha256")
  summary+="| \`$f\` | $(du -h "$out/$f" | cut -f1) | \`$(cut -d' ' -f1 "$out/$f.sha256")\` |\n"
done
all=()
for f in "${files[@]}"; do all+=("$out/$f" "$out/$f.sha256"); done
say "made:"
printf '  %s\n' "${all[@]}"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  { echo "bundle=$bundle"; echo "dir=$out"; echo "files<<NAMI_EOF"; printf '%s\n' "${all[@]}"; echo "NAMI_EOF"; } >> "$GITHUB_OUTPUT"
fi

# 4. Onto the release: the one the workflow runs for, or the tag's.
tag=${INPUT_TAG:-}
if [ -z "$tag" ] && [ "${GITHUB_REF:-}" != "${GITHUB_REF#refs/tags/}" ]; then
  tag=${GITHUB_REF#refs/tags/}
fi
upload=${INPUT_UPLOAD:-auto}
case ${upload,,} in
  auto) [ -n "$tag" ] && upload=true || { upload=false; say "not uploading: this run isn't for a release or a tag (upload: true and tag: to say which)"; } ;;
  true|yes|1) upload=true; [ -n "$tag" ] || fail "upload: true, but there's no tag to upload to: say which with tag:" ;;
  *) upload=false ;;
esac
if [ "$upload" = true ]; then
  group "Uploading to the release $tag"
  command -v gh >/dev/null || fail "uploading needs the GitHub CLI (gh), which GitHub's runners have"
  [ -n "${GH_TOKEN:-}" ] || fail "uploading needs a token that can write releases (token:, and permissions: contents: write)"
  repo=(); [ -z "${GITHUB_REPOSITORY:-}" ] || repo=(--repo "$GITHUB_REPOSITORY")
  if ! gh release view "$tag" "${repo[@]}" >/dev/null 2>&1; then
    say "no release for $tag yet: making it"
    gh release create "$tag" "${repo[@]}" --verify-tag --title "$tag" --generate-notes
  fi
  gh release upload "$tag" "${repo[@]}" --clobber "${all[@]}"
  endgroup
  summary+="\nUploaded to the release **$tag**."
fi
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  printf '### nami bundle\n\n%b\n\nRuns on any x86_64 Linux with glibc: download, `chmod +x %s`, run.\n' "$summary" "$name" >> "$GITHUB_STEP_SUMMARY"
fi
