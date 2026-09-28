#!/bin/sh
# Separate build for PocketBook Pro 903 (FW2, Samsung S3C6410 = ARMv6).
# Runs INSIDE the 903 container (see Containerfile.pocketbook-903).
# Usage (from repo root, Windows PowerShell):
#   podman build -f Containerfile.pocketbook-903 -t pbweb-sdk-903 .
#   podman run --rm -v ${PWD}:/work -w /work pbweb-sdk-903 sh build-pocketbook-903.sh
# Result: target/pbweb-903.app
#   (copy to /mnt/ext1/applications/pbweb-903.app on the reader,
#   icons to /mnt/ext1/applications/icons/ like the 633 build)
set -eu
FRSCSDK="${PBSDK_ROOT:-/opt/pbsdk/FRSCSDK}"
export PATH="$FRSCSDK/bin:$PATH"
echo "FRSCSDK=$FRSCSDK"
which arm-none-linux-gnueabi-gcc
# fail fast: 2008-era 32-bit toolchain needs ia32 compat (see Containerfile)
arm-none-linux-gnueabi-gcc --version | head -n 3

# --- link-time stub for libinkview.so ---
# FRSCSDK ships headers but no ARM libinkview (linking happens against the
# real library on the device at load). This stub only resolves the names we
# hard-link under the pro903 feature and records SONAME libinkview.so.
# Deliberately NO DialogSynchro / SendEventTo here: pro903 code must not
# reference them, and the link breaks loudly if it ever does.
# NOTE: the stub lives in /tmp (container-local), NOT in /work: the 2008-era
# 32-bit cc1 fails with EOVERFLOW ("Value too large for defined data type")
# when reading sources off the Windows-mounted /work volume. Rust (64-bit)
# is unaffected and keeps using /work/target.
STUBDIR="${STUBDIR:-/tmp/inkview-stub-903}"
mkdir -p "$STUBDIR"
cat > "$STUBDIR/inkview_stub.c" <<'EOF'
/* Link-time only stub for FW2 libinkview. Bodies never execute. */
typedef void *iv_ptr;
typedef const char *iv_str;
void InkViewMain(void *h) { (void)h; }
void CloseApp(void) {}
int ScreenWidth(void) { return 0; }
int ScreenHeight(void) { return 0; }
void ClearScreen(void) {}
void FullUpdate(void) {}
void PartialUpdate(int x, int y, int w, int h) { (void)x; (void)y; (void)w; (void)h; }
void FillArea(int x, int y, int w, int h, int c) { (void)x; (void)y; (void)w; (void)h; (void)c; }
void DrawRect(int x, int y, int w, int h, int c) { (void)x; (void)y; (void)w; (void)h; (void)c; }
void SetPanelType(int t) { (void)t; }
iv_ptr OpenFont(iv_str n, int s, int a) { (void)n; (void)s; (void)a; return (iv_ptr)0; }
void CloseFont(iv_ptr f) { (void)f; }
void SetFont(iv_ptr f, int c) { (void)f; (void)c; }
char *DrawTextRect(int x, int y, int w, int h, iv_str s, int fl) {
    (void)x; (void)y; (void)w; (void)h; (void)s; (void)fl; return (char *)0;
}
void DrawString(int x, int y, iv_str s) { (void)x; (void)y; (void)s; }
void Message(int ic, iv_str t, iv_str tx, int to) { (void)ic; (void)t; (void)tx; (void)to; }
void Dialog(int ic, iv_str t, iv_str tx, iv_str b1, iv_str b2, void *cb) {
    (void)ic; (void)t; (void)tx; (void)b1; (void)b2; (void)cb;
}
int QueryNetwork(void) { return 0; }
int NetConnect(iv_str n) { (void)n; return 0; }
void NetDisconnect(void) {}
char *GetHwAddress(void) { return (char *)0; }
void iv_sync(void) {}
void SendEvent(void *h, int t, int p1, int p2) { (void)h; (void)t; (void)p1; (void)p2; }
EOF
arm-none-linux-gnueabi-gcc -shared -fPIC -Os -march=armv5te \
    -Wl,-soname,libinkview.so \
    -o "$STUBDIR/libinkview.so" "$STUBDIR/inkview_stub.c"

# --- FW2 (glibc 2.5) compat shims for symbols modern Rust std expects ---
# glibc 2.5 has no accept4 / getauxval / pthread_setname_np, and gcc 4.1's
# libgcc_s has no _Unwind_Backtrace: without these the link fails with
# "undefined reference". Semantics match newer glibc; thread naming and
# backtraces are cosmetic here (panic=abort, std ignores naming failures).
cat > "$STUBDIR/fw2_compat.c" <<'EOF'
/* FW2 compat shims. Only linked into the Pro 903 binary. */
#define _GNU_SOURCE
#include <sys/types.h>
#include <sys/socket.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdio.h>
#include <elf.h>
#include <pthread.h>
#include <unwind.h>

#ifndef SOCK_CLOEXEC
#define SOCK_CLOEXEC 02000000
#endif
#ifndef SOCK_NONBLOCK
#define SOCK_NONBLOCK 04000
#endif

int accept4(int sockfd, struct sockaddr *addr, socklen_t *addrlen, int flags) {
    int fd = accept(sockfd, addr, addrlen);
    if (fd < 0) return -1;
    if (flags & SOCK_CLOEXEC) fcntl(fd, F_SETFD, FD_CLOEXEC);
    if (flags & SOCK_NONBLOCK) {
        int fl = fcntl(fd, F_GETFL, 0);
        if (fl >= 0) fcntl(fd, F_SETFL, fl | O_NONBLOCK);
    }
    return fd;
}

unsigned long getauxval(unsigned long type) {
    FILE *f = fopen("/proc/self/auxv", "rb");
    if (!f) return 0;
    Elf32_auxv_t av;
    unsigned long val = 0;
    while (fread(&av, sizeof av, 1, f) == 1) {
        if (av.a_type == AT_NULL) break;
        if (av.a_type == type) { val = av.a_un.a_val; break; }
    }
    fclose(f);
    return val;
}

int pthread_setname_np(pthread_t thread, const char *name) {
    (void)thread; (void)name;
    return 0;
}

_Unwind_Reason_Code _Unwind_Backtrace(void *trace, void *ref) {
    (void)trace; (void)ref;
    return _URC_END_OF_STACK;
}
EOF
arm-none-linux-gnueabi-gcc -c -Os -march=armv5te \
    -o "$STUBDIR/fw2_compat.o" "$STUBDIR/fw2_compat.c"

# --- Rust codegen for ARM1176JZF-S (ARMv6, soft-float ABI like glibc 2.5) ---
# .cargo/config.toml only sets linker+link-args; the CPU comes from here
# because the 633 build shares this target triple and needs cortex-a7.
# The linker override is also here: FRSCSDK gcc, not the obreey clang.
export RUSTFLAGS="-C target-cpu=arm1176jzf-s -C link-arg=-L$STUBDIR -C link-arg=$STUBDIR/fw2_compat.o -C link-arg=-static-libgcc"
export CARGO_TARGET_ARM_UNKNOWN_LINUX_GNUEABI_LINKER="arm-none-linux-gnueabi-gcc"

cargo build --release --target arm-unknown-linux-gnueabi \
    -p pbweb-app --features pbweb-app/device,pro903,keyonly
echo "OK: target/arm-unknown-linux-gnueabi/release/pbweb-app -> rename to pbweb-903.app for device"
cp -f target/arm-unknown-linux-gnueabi/release/pbweb-app target/pbweb-903.app || true
# Old ld 2.17 keeps the non-alloc .llvmbc section (LLVM bitcode, ~7.8MB)
# that modern linkers drop — strip it (in /tmp: 32-bit strip can't
# stat files on the Windows-mounted /work, same EOVERFLOW as cc1).
STRIP_TMP=/tmp/pbweb-903-strip.app
cp -f target/pbweb-903.app "$STRIP_TMP"
arm-none-linux-gnueabi-strip -R .llvmbc -R .comment -R .ARM.attributes "$STRIP_TMP"
cp -f "$STRIP_TMP" target/pbweb-903.app
ls -lh target/pbweb-903.app || true
# --- smoke checks: wrong CPU arch or FW6-only imports fail the build ---
# NOTE: 32-bit readelf/nm can't stat files on the Windows-mounted /work
# (same EOVERFLOW as cc1 above), so check a /tmp copy.
SMOKE_TMP=/tmp/pbweb-903-smoke.app
cp -f target/pbweb-903.app "$SMOKE_TMP"
echo "-- arch attributes (informational; old readelf may show little) --"
arm-none-linux-gnueabi-readelf -A "$SMOKE_TMP" | head -n 12 || true
echo "-- ARMv7-only instruction scan (informational) --"
echo "   (stripped binary lost \$d mapping symbols, so old objdump also"
echo "   decodes literal pools: the known '_BAC[RUST...]' string data at"
echo "   0x9d618 decodes as 'movtmi'. Any OTHER movw/movt hit = real bug.)"
arm-none-linux-gnueabi-objdump -d "$SMOKE_TMP" | grep -e movw -e movt || echo "   no movw/movt text hits at all"
arm-none-linux-gnueabi-readelf -l "$SMOKE_TMP" | grep -i "interpreter" || true
arm-none-linux-gnueabi-readelf -d "$SMOKE_TMP" | grep -i "NEEDED" || true
echo "-- dynamic imports --"
arm-none-linux-gnueabi-nm -D --undefined-only "$SMOKE_TMP" | sort
# FW6-only symbols must NEVER be hard imports (dlsym strings are fine).
if arm-none-linux-gnueabi-nm -D --undefined-only "$SMOKE_TMP" | grep -q "DialogSynchro\|SendEventTo\|NetConnectAsync\|NetConnectSilent\|NetInfo\|GetTouchInfo\|ivmpc"; then
    echo "FATAL: FW6-only symbol referenced by pbweb-903.app"
    exit 1
fi
echo "smoke OK: no FW6-only imports"
