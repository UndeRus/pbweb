# Building PBWeb

PBWeb ships two artifacts:
- `target/pbweb.app` — PocketBook 633 (Cortex-A7, firmware 6.x).
- `target/pbweb-903.app` — PocketBook Pro 903 (ARM1176, firmware 2.x).

## Host development (Windows, no device needed)

```powershell
cargo test --workspace
cargo run -p pbweb-app
# pairing PIN is printed to the console on start
# open http://127.0.0.1:8080  (serves repo dir)
curl.exe -F "files=@book.epub" "http://127.0.0.1:8080/api/upload?path=int:/&token=123456"
curl.exe "http://127.0.0.1:8080/api/download?path=int:/book.epub&token=123456" -o out.epub
```

## Cross build (Podman) — 633

```powershell
podman build -f Containerfile.pocketbook -t pbweb-sdk .
podman run --rm -v ${PWD}:/work -w /work pbweb-sdk sh build-pocketbook.sh
# -> target/pbweb.app  (copy to /mnt/ext1/applications/pbweb.app on reader)
```

Container: SDK 6.5 branch + Rust `arm-unknown-linux-gnueabi`. Linker
`arm-obreey-linux-gnueabi-clang`, cortex-a7 (see `.cargo/config.toml`).

## Pro 903 build (separate artifact, FW2 / ARMv6)

Pro 903 (Samsung S3C6410 = ARM1176JZF-S, 1200x825 landscape, stylus,
firmware 2.x) cannot run the 633 binary: different CPU (ARMv6 vs Cortex-A7),
different glibc (2.5) and a different InkView (no `DialogSynchro`,
`NetConnectAsync`, `NetInfo`, or library scan via `EVT_STARTSCAN`). Hence a
separate container with the old FRSCSDK (gcc 4.1.2 + glibc 2.5) and a
separate artifact:

```powershell
podman build -f Containerfile.pocketbook-903 -t pbweb-sdk-903 .
podman run --rm -v ${PWD}:/work -w /work pbweb-sdk-903 sh build-pocketbook-903.sh
# -> target/pbweb-903.app  (copy to /mnt/ext1/applications/pbweb-903.app)
```

Technique: features `pbweb-app/device,pro903,keyonly` — Rust code targeting
`arm1176jzf-s`, linker `arm-none-linux-gnueabi-gcc`, `libinkview.so` replaced
by a link-time stub (the real library resolves at load time on the device;
`DialogSynchro`/`SendEventTo` are deliberately absent from the stub so linking
fails loudly if code touches them). CPU/linker are set in
`build-pocketbook-903.sh`, not in `.cargo/config.toml`: both builds share the
`arm-unknown-linux-gnueabi` triple.

glibc 2.5 vs modern Rust std: `build-pocketbook-903.sh` embeds compat shims
(`accept4` via `accept`+`fcntl`, `getauxval` via `/proc/self/auxv`,
`pthread_setname_np` as no-op, `_Unwind_Backtrace` as stub — backtraces are
not used, `panic=abort`). Verified: interpreter `/lib/ld-linux.so.3`, NEEDED
only era libraries (`libinkview`, `libdl`, `libgcc_s`, `librt`, `libpthread`,
`libc`), no FW6 symbols in imports (smoke test in the script), no ARMv7
instructions (`movw`/`movt`) in code (the single scan hit is bytes of the
string literal `..._BAC[RUST...]` in the literal pool, not an instruction).

Pro 903 build differences (verified against FRSCSDK headers `inkview.new.h`):

- WiFi: only blocking `NetConnect(NULL)` in a worker (+ `silent` if the
  firmware exports it); state polled via `QueryNetwork()`, `NetInfo` untouched
  (opaque struct in old headers).
- WiFi prompt: async `Dialog` + callback (no blocking `DialogSynchro` on FW2).
- STOP/EXIT do not send `EVT_STARTSCAN` (no scanner service on FW2) — files
  are on disk anyway; refresh Library manually.
- Network events `EVT_NET_*` never arrive on FW2 — polling only.

## See also

- [architecture.md](architecture.md) — crate map and implementation details.
- [troubleshooting-903.md](troubleshooting-903.md) — Pro 903 field checklist.
