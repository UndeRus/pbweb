#!/bin/sh
# Build inside podman container with SDK on PATH.
# Usage (from repo root, Windows PowerShell):
#   podman build -f Containerfile.pocketbook -t pbweb-sdk .
#   podman run --rm -v ${PWD}:/work -w /work pbweb-sdk sh build-pocketbook.sh
set -eu
SDK_SYSROOT="$SDK_ROOT_DIR/SDK-B288/usr/arm-obreey-linux-gnueabi/sysroot"
export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$SDK_SYSROOT -I$SDK_SYSROOT/usr/include/freetype2"
export PATH="$SDK_ROOT_DIR/SDK-B288/usr/bin:$PATH"
# CPU tuning lives here (not .cargo/config.toml): the 633 build shares its
# target triple with the Pro 903 build, which needs ARMv6 instead.
export RUSTFLAGS="-C target-cpu=cortex-a7"
echo "SDK_ROOT_DIR=$SDK_ROOT_DIR"
which arm-obreey-linux-gnueabi-clang
# fail fast: old SDK wrapper needs libtinfo.so.5 compat (see Containerfile)
arm-obreey-linux-gnueabi-clang --version | head -n 3
cargo build --release --target arm-unknown-linux-gnueabi -p pbweb-app --features pbweb-app/device
echo "OK: target/arm-unknown-linux-gnueabi/release/pbweb-app -> rename to pbweb.app for device"
cp -f target/arm-unknown-linux-gnueabi/release/pbweb-app target/pbweb.app || true
ls -lh target/pbweb.app || true
