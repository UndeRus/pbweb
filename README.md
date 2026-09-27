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

## Install on reader (+ icon)

1. Copy `target/pbweb.app` → `/mnt/ext1/applications/pbweb.app`.
2. Copy icons (8-bit BMP, e-ink style: WiFi + open book):
   `assets/pbweb.bmp` → `/mnt/ext1/applications/icons/pbweb.bmp`,
   `assets/pbweb_f.bmp` → `/mnt/ext1/applications/icons/pbweb_f.bmp`
   (the `_f` one is the tapped/inverted state).
3. Optional launcher entry — in `/system/config/desktop/view.json` add:
   `"U_pbweb": { "path": "/mnt/ext1/applications/pbweb.app", "title": "PBWeb",
   "icon": "/mnt/ext1/applications/icons/pbweb.bmp",
   "focused_icon": "/mnt/ext1/applications/icons/pbweb_f.bmp" },`
   and add `"U_pbweb"` to a group. Reboot/unplug to refresh.
   Regenerate icons anytime: `python assets/make_icons.py`.

![PBWeb icon](assets/pbweb-preview.png)

Device run: open `pbweb.app` → big СТАРТ button (or MENU key) → Yes in the
WiFi prompt → non-blocking connect (silent first, then async, 45s cap with
countdown) → huge URL + steps on screen. All in Russian, all actions are big
touch buttons: СТАРТ/СТОП, ЭКРАН (cycle tabs), ВЫХОД; tapping a file row opens it.

- СТОП really stops the server (socket closed, accept loop unblocked) — WiFi
  stays on, СТАРТ works again instantly.
- Upload progress on the reader screen: file name, percent, speed (МБ/с),
  progress bar; finished uploads stay as "✓ name (size)" until the next one.
- Log tab shows recent HTTP requests served.
- No top status bar; no blocking `NetConnect*` calls — the UI never hangs.

WiFi details: online = `NetInfo()->connected` only; `NetConnectSilent` then
`NetConnectAsync` + poll; `EVT_NET_CONNECTED` handling; real IP via `getifaddrs`
(wlan* > eth* > any). Touch: `POINTERDOWN`/`TOUCHDOWN` (47/48/49) with x/y from
event params, PocketPuzzles-style (no `GetTouchInfo` indirection; 6.5 lib only
has `GetTouchInfoI`).

Stop with the ВЫХОД button or BACK: sync + library nudge + exit.

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
