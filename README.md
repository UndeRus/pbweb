# PBWeb — WiFi file manager for PocketBook 633 (Rust)

- On device: raises WiFi (`NetConnect2`/`NetConnect` fallback), shows URL on e-ink screen, serves files.
- From browser (same WiFi): Crosspoint-style UI (Home status + File Manager) over
  `/mnt/ext1` (+ `/mnt/ext2` SD if present): browse, upload (drag&drop + progress),
  download, folder-as-zip, mkdir/rename/move/delete (multi-select).
- Single self-contained `web/index.html` (~19KB, no CDN — works in LAN without internet).
- After stop: `iv_sync` + explorer-db mtime poke (best-effort library nudge; real rescan happens on Library open — see note below).

## Layout

```
Containerfile.pocketbook   Podman image: SDK 6.5 branch + Rust arm-unknown-linux-gnueabi
build-pocketbook.sh        runs INSIDE container
.cargo/config.toml         linker arm-obreey-linux-gnueabi-clang, cortex-a7
crates/pb-sys   minimal InkView FFI (no bindgen). Old+new FW compat via dlsym
crates/pb-core  safe roots, listing, traversal guard (host-testable)
crates/pb-server tiny_http routes + multipart upload (host-testable)
crates/pb-ui    e-ink UI state (tabs, paging math)
crates/pb-app   main: InkViewMain on device / plain server on host
web/index.html  embedded via include_str!
```

## Host dev (Windows, no device needed)

```powershell
cargo test --workspace
cargo run -p pbweb-app
# open http://127.0.0.1:8080  (serves repo dir)
curl.exe -F "files=@book.epub" "http://127.0.0.1:8080/api/upload?path=int:/"
curl.exe "http://127.0.0.1:8080/api/download?path=int:/book.epub" -o out.epub
```

## Cross build (Podman)

```powershell
podman build -f Containerfile.pocketbook -t pbweb-sdk .
podman run --rm -v ${PWD}:/work -w /work pbweb-sdk sh build-pocketbook.sh
# -> target/pbweb.app  (copy to /mnt/ext1/applications/pbweb.app on reader)
```

Device run: open `pbweb.app` from apps list → big START button (or MENU key)
→ Yes in the WiFi prompt → non-blocking connect (45s cap, countdown on screen)
→ URL in huge type. All actions are big touch buttons: START/STOP, UP, EXIT,
top tab bar; tapping a file row opens the folder.

WiFi details: `NetConnectSilent` first (no dialogs), then `NetConnectAsync` +
`NetInfo` polling, `EVT_NET_CONNECTED` handling, real IP via `getifaddrs`.
Online = `NetInfo()->connected` only (`QueryNetwork()` semantics are undocumented).
Touch: `POINTERDOWN`/`TOUCHDOWN` (47/48/49) with x/y from event params,
PocketPuzzles-style (no `GetTouchInfo` indirection; 6.5 lib only has `GetTouchInfoI`).
No blocking `NetConnect*` calls — the UI never hangs.

Stop with the EXIT button or BACK: sync + library nudge + exit.

Diagnostics: the app appends to `/mnt/ext1/pbweb.log` (startup, key/touch codes,
WiFi steps, panics). If something crashes, send this file — it tells exactly where.

## API

- `GET /` , `GET /files` — web UI (SPA: Home + File Manager)
- `GET /api/status` — `{"version","ip","mode","device","wifi","roots"}`
- `GET /api/roots` — `{"internal":true,"sdcard":bool}`
- `GET /api/files?path=int:/books` or `sd:/...`
- `GET /api/download?path=...`
- `GET /api/zip?path=int:/dir` — folder as .zip (stored, 2000 files / 512MB caps)
- `POST /api/mkdir` `{"path","name"}` / `POST /api/rename` `{"path","name"}`
- `POST /api/move` `{"path","dest"}` (same root only)
- `POST /api/delete` `{"path"}` or `{"paths":[...]}` (files + empty folders only)
- `POST /api/upload?path=int:/dir` multipart `files=@...` (512MB cap)

## Notes / limits (v1)

- IPs: shown as placeholder until first route; fill from your router's client list if `192.168.0.?`.
  Improvement: read `NetInfo().prefix` via dlsym + `getifaddrs` — queued.
- Library rescan: no public `RescanLibrary()` in 5.x/6.x headers found.
  Current: `iv_sync()` + touch `explorer-3.db` mtime. Confirmed full rescan path needs
  `strings libinkview.so` spike inside container (symbols `UpdateBookInfo*`, `Scan*`, `BookReady`).
- Device UI v1: status screen + tabs state; Files/Log full on-device rendering is next.
- Port: 8080..8090 auto-fallback. No auth in v1 — trusted LAN only.
