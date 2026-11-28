#!/bin/sh
# Turns the release tarball into a .deb and an .rpm: installers people
# double-click (GNOME Software, Discover, ... ask for the password; no
# terminal). Both put the release in /opt/nami and run its setup.sh, which
# downloads the signed catalog; removing the
# package runs uninstall.sh. No dependencies: nami is one static binary.
# Needs dpkg-deb and rpmbuild (nix shell nixpkgs#dpkg nixpkgs#rpm).
# nami was tray: the packages provide tray, but don't conflict with tray's
# or obsolete it. Either would have apt or dnf remove tray's package first,
# and its removal runs tray's uninstall.sh, which takes the store and every
# app with it. Instead setup.sh makes that uninstall.sh say so and do nothing,
# and tray's package can then be removed by hand, safely
# (docs/rename-migration.md).
# usage: BASE_URL=... CHANNEL=... KEY=... packaging/distro-packages.sh <release-tarball> <out-dir>
set -eu
: "${BASE_URL:?}" "${CHANNEL:?}" "${KEY:?}"
tarball=$(realpath "${1:?release tarball}")
out=$(realpath -m "${2:?out dir}")
version=$(basename "$tarball" | sed -n 's/^nami-\(.*\)-x86_64-linux\.tar\.gz$/\1/p')
[ -n "$version" ] || { echo "not a release tarball: $tarball"; exit 1; }
summary="Package manager for any distribution: your computer, your apps and your projects"
mkdir -p "$out"
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
tar -xzf "$tarball" -C "$work"
src=$work/nami-$version

postinst="NAMI_CHANNEL='$BASE_URL/menu/$CHANNEL' NAMI_KEY='$KEY' sh /opt/nami/setup.sh"
prerm="sh /opt/nami/uninstall.sh"

echo "== deb"
d=$work/deb
mkdir -p "$d/opt" "$d/DEBIAN"
cp -a "$src" "$d/opt/nami"
cat > "$d/DEBIAN/control" <<EOF
Package: nami
Version: $version
Provides: tray
Architecture: amd64
Maintainer: Nami project
Section: admin
Priority: optional
Homepage: $BASE_URL
Description: $summary
 Installs nami and the signed package catalog.
EOF
printf '#!/bin/sh\nset -e\n%s\n' "$postinst" > "$d/DEBIAN/postinst"
# Only on removal: an upgrade keeps nami and the apps it installed.
printf '#!/bin/sh\nset -e\nif [ "$1" = remove ] || [ "$1" = purge ]; then %s; fi\n' "$prerm" > "$d/DEBIAN/prerm"
chmod 755 "$d/DEBIAN/postinst" "$d/DEBIAN/prerm"
dpkg-deb --root-owner-group -Zxz --build "$d" "$out/nami_${version}_amd64.deb" >/dev/null
echo "$out/nami_${version}_amd64.deb"

echo "== rpm"
r=$work/rpm
mkdir -p "$r/SPECS" "$r/BUILDROOT" "$r/RPMS"
cat > "$r/SPECS/nami.spec" <<EOF
Name: nami
Version: $(echo "$version" | tr - _)
Provides: tray
Release: 1
Summary: $summary
License: MIT
URL: $BASE_URL
BuildArch: x86_64
AutoReqProv: no
%define _build_id_links none
%define __strip /bin/true
%define debug_package %{nil}

%description
Installs nami and the signed package catalog.

%install
mkdir -p %{buildroot}/opt
cp -a $src %{buildroot}/opt/nami

%files
/opt/nami

%post
$postinst

%preun
# 0: removal (1 would be an upgrade).
if [ "\$1" = 0 ]; then $prerm; fi
EOF
rpmbuild --quiet -bb --define "_topdir $r" --define "_tmppath $r/tmp" --define "_dbpath $r/db" "$r/SPECS/nami.spec"
cp "$r"/RPMS/x86_64/nami-*.rpm "$out/nami-${version}-1.x86_64.rpm"
echo "$out/nami-${version}-1.x86_64.rpm"
