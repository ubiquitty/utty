#!/bin/sh
# Copyright (C) 2026 Stanislav Yaglo
# SPDX-License-Identifier: Apache-2.0
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
git submodule update --init ThirdParty/ubiquitty-core
build_type=${BUILD_TYPE:-Release}
jobs=${JOBS:-4}
case "$build_type" in
    Debug) profile=debug; release=; sdk="$root/.sdk/split-core" ;;
    Release) profile=release; release=--release; sdk="$root/.sdk/split-core-release" ;;
    *) echo "BUILD_TYPE must be Debug or Release" >&2; exit 2 ;;
esac
# The pinned upstream Terminal exceeds its size budget with libstdc++.
# libc++ stays within it. Both the SDK and the client must use the same ABI.
if [ "$(uname -s)" = Linux ]; then
    export CC=${CC:-clang} CXX=${CXX:-clang++}
    flags=-stdlib=libc++
else
    flags=
fi
cmake -S ThirdParty/ubiquitty-core -B "$sdk/core" -G Ninja \
    -DUBIQUITTY_FRONTEND=Core -DUBIQUITTY_QUIC=OFF \
    -DCMAKE_BUILD_TYPE="$build_type" -DCMAKE_CXX_FLAGS="$flags" \
    -DCMAKE_INSTALL_LIBDIR=lib \
    -DCMAKE_INSTALL_PREFIX="$sdk/install"
cmake --build "$sdk/core" --target ubiquitty-kit -j "$jobs"
cmake --install "$sdk/core"
export UBIQUITTY_CORE_SDK="$sdk/install"
cargo build --locked ${release:+"$release"} -j "$jobs"
cargo test --locked ${release:+"$release"} -j "$jobs"
mkdir -p build
cp "target/$profile/utty" build/utty.next
mv build/utty.next build/utty
cp "target/$profile/utty-capture" build/utty-capture.next
mv build/utty-capture.next build/utty-capture
if [ "${1:-}" = --host ]; then
    host_source="$root/ThirdParty/ubiquitty-core"
    host_build="$sdk/host-build"
    cmake -S "$host_source" -B "$host_build" -G Ninja \
        -DUBIQUITTY_FRONTEND=Host -DUBIQUITTY_QUIC=OFF \
        -DCMAKE_BUILD_TYPE="$build_type" -DCMAKE_CXX_FLAGS="$flags"
    cmake --build "$host_build" --target ubiquitty-host -j "$jobs"
    mkdir -p "$root/.sdk/host"
    cp "$host_build/ubiquitty-host" "$root/.sdk/host/ubiquitty-host.new"
    mv "$root/.sdk/host/ubiquitty-host.new" "$root/.sdk/host/ubiquitty-host"
    # A client without ~/.ubiquitty/ubiquitty-host finds the host beside itself.
    for directory in build "target/$profile"; do
        cp "$host_build/ubiquitty-host" "$directory/ubiquitty-host.next"
        mv "$directory/ubiquitty-host.next" "$directory/ubiquitty-host"
    done
fi
