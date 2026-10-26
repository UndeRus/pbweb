# Architecture and implementation details

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

## Button key routing

Every bottom-bar action is reachable from the keyboard — routing lives in
`pb-ui::key_action` (a pure function with host tests: `cargo test -p pb-ui`).
The `keyonly` feature (enabled in the 903 build) switches on-screen hints to
button-oriented wording; the routing itself works in every build, including 633.

## WiFi

Online state is `NetInfo()->connected` only; connect via `NetConnectSilent`
then `NetConnectAsync` + poll; `EVT_NET_CONNECTED` handling; real IP via
`getifaddrs` (wlan* > eth* > any). No blocking `NetConnect*` calls — the UI
never hangs.

## Touch input

`POINTERDOWN`/`TOUCHDOWN` (47/48/49) with x/y taken from event params,
PocketPuzzles-style (no `GetTouchInfo` indirection; the 6.5 lib only has
`GetTouchInfoI`).

## Screen updates

Partial screen updates (PocketPuzzles pattern): taps, selection moves,
progress ticks, countdown, WiFi/scan messages and BACK navigation repaint
only their rects via `PartialUpdate`; `FullUpdate` runs on tab switches,
server transitions and every 10th partial (anti-ghosting).

## Library rescan

On STOP and EXIT the app broadcasts `EVT_STARTSCAN` to the resident
`scanner.app` service — exactly what the firmware's own
`PBScanClient::startDeviceScan()` does (`SendEventTo(-3, 0xD7, 0, 0)`).
Completion arrives as `EVT_SCANSTOPPED` ("Library updated" on screen); scan
policy (`scanmode`) is only read for the log, never changed. No `EVT_STOPSCAN`
— the firmware answers it "Not Implemented". On FW2 (Pro 903) no scan is
sent: there is no scanner service; files are on disk and Library is refreshed
manually.

## Diagnostics

The app appends to `/mnt/ext1/pbweb.log` (startup, key/touch codes, WiFi
steps, panics). If something crashes, send this file — it tells exactly where.
