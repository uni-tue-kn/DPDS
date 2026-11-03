#!/usr/bin/env bash
# Builds a minimal DPDK from the official release tarball and installs it into PREFIX.
# Used by .github/workflows/ci.yml; the workflow caches PREFIX keyed on the hash of THIS file,
# so changing the version or any build option below rebuilds DPDK.
#
# Usage: install-dpdk.sh PREFIX
# Needs: meson, ninja, python3-pyelftools, libpcap-dev, a C compiler, curl, xz.
#
# PREFIX must not be a world-writable directory (e.g. below /tmp): DPDK refuses to load its
# driver plugins from there ("EAL: Error, directory path /tmp is world-writable and insecure").
set -euo pipefail

PREFIX="${1:?usage: install-dpdk.sh PREFIX}"

VERSION=26.07
# sha256 of https://fast.dpdk.org/rel/dpdk-26.07.tar.xz (its md5 cc780f20062614359fb1b8de6068c454
# equals the one listed on https://core.dpdk.org/download/)
SHA256=7141a8b5bad9d7d965483ac0d75317ac0c21dcee1d13d373693c655f9e3fabe6

# Build options:
# - platform=generic: not tuned for the build host's CPU, so the cached install also runs on a
#   runner with another CPU model (the default `native` would not).
# - enable_drivers: only what the tests use. net_null and net_pcap are the vdevs of the e2e tests,
#   net_ring is accepted by the controller, mempool/ring is the default mempool.
#   The bus/vdev driver is pulled in as a dependency.
# - apps, tests and docs are not needed (the libraries and headers are all dpdk-sys links against).
# - libdir=lib: the same path on every architecture (default is lib/<multiarch triplet>).
OPTIONS=(
  --buildtype=release
  --libdir=lib
  -Dplatform=generic
  -Dtests=false
  -Denable_docs=false
  '-Ddisable_apps=*'
  -Denable_drivers=net/null,net/pcap,net/ring,mempool/ring
)

# The workflow sets CFLAGS for the Rust build (-march=x86-64-v2); DPDK picks its own flags.
unset CFLAGS

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

curl --fail --silent --show-error --location --retry 3 \
  --output "dpdk-$VERSION.tar.xz" "https://fast.dpdk.org/rel/dpdk-$VERSION.tar.xz"
echo "$SHA256  dpdk-$VERSION.tar.xz" | sha256sum --check --strict -
tar -xf "dpdk-$VERSION.tar.xz"

meson setup "dpdk-$VERSION/build" "dpdk-$VERSION" --prefix="$PREFIX" "${OPTIONS[@]}"
ninja -C "dpdk-$VERSION/build" install
