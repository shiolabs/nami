#!/bin/sh
# Makes the "prebuilt" release tarball the way a project without nami would:
# with the distro's own gcc, a program and a library of its own beside it.
# usage: build-on-ubuntu.sh <out-dir>   (in an Ubuntu container with gcc)
set -eu
src=$(dirname "$0")
d=$(mktemp -d)/prebuilt-hello-1.2.3
mkdir -p "$d/bin" "$d/lib"
gcc -shared -fPIC -o "$d/lib/libgreet.so" "$src/greet.c"
gcc -o "$d/bin/prebuilt-hello" "$src/hello.c" -L"$d/lib" -lgreet -Wl,-rpath,'$ORIGIN/../lib'
tar -czf "$1/prebuilt-hello-1.2.3-linux-x86_64.tar.gz" -C "$(dirname "$d")" prebuilt-hello-1.2.3
