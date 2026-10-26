# Pro 903 field testing and firmware-2 specifics

Pro 903 (Samsung S3C6410, ARM1176JZF-S, 1200x825 landscape, stylus,
firmware 2.x) differs from 633 in several ways — see
[build.md](build.md) for the build-side details. This page is the runtime
checklist.

## Field checklist (no stylus, buttons + `pbweb.log`)

Status OK → start → OK → stop; Files UP/DOWN + OK + BACK to root → Status →
BACK to exit; PREV/NEXT page. Send `/mnt/ext1/pbweb.log` — it contains key
codes, WiFi steps and `query_network=`.

## Device-side risk checks

- `ls /lib/libgcc_s*` (binary links `libgcc_s.so.1` dynamically).
- `ls /usr/local/lib/libinkview*` (which InkView library we link against).

## Firmware-2 specifics (verified against FRSCSDK `inkview.new.h`)

- WiFi: only blocking `NetConnect(NULL)` in a worker (+ `silent` if the
  firmware exports it); state via `QueryNetwork()`, `NetInfo` untouched
  (opaque struct in old headers).
- WiFi prompt: async `Dialog` + callback (no blocking `DialogSynchro` on FW2).
- STOP/EXIT do not send `EVT_STARTSCAN` (no scanner service on FW2) — files
  are on disk anyway; refresh Library manually.
- Network events `EVT_NET_*` never arrive on FW2 — polling only.
- Library rescan is not triggered (see [architecture.md](architecture.md)).
