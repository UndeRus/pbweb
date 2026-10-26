# Installing PBWeb on the reader

PBWeb ships two artifacts:
- `target/pbweb.app` — PocketBook 633 (Cortex-A7, firmware 6.x).
- `target/pbweb-903.app` — PocketBook Pro 903 (ARM1176, 1200x825 landscape,
  stylus, firmware 2.x).

See [build.md](build.md) for how to build them (or download a release).

## Install

1. Copy the artifact → `/mnt/ext1/applications/pbweb.app`
   (`pbweb-903.app` for Pro 903).
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

After STOP or EXIT the app asks the firmware to rescan the library (best
effort, 0.2.6+); metadata appears when you open Library.

## Notes and limits (v1)

- IPs are shown as a placeholder until the first request; if it stays
  `192.168.0.?`, take the address from your router's client list.
- Port is 8080..8090 with automatic fallback.
- The server runs only while you explicitly started it, plus a 15-min idle
  auto-stop. See [security.md](security.md) for the full model.

## Diagnostics

The app appends to `/mnt/ext1/pbweb.log` (startup, key/touch codes, WiFi
steps, panics). If something crashes, send that file — it tells exactly
where. For the Pro 903 field checklist see
[troubleshooting-903.md](troubleshooting-903.md).
