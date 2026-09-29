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
# pairing PIN is printed to the console on start
# open http://127.0.0.1:8080  (serves repo dir)
curl.exe -F "files=@book.epub" "http://127.0.0.1:8080/api/upload?path=int:/&token=123456"
curl.exe "http://127.0.0.1:8080/api/download?path=int:/book.epub&token=123456" -o out.epub
```

## Cross build (Podman)

```powershell
podman build -f Containerfile.pocketbook -t pbweb-sdk .
podman run --rm -v ${PWD}:/work -w /work pbweb-sdk sh build-pocketbook.sh
# -> target/pbweb.app  (copy to /mnt/ext1/applications/pbweb.app on reader)
```

## Pro 903 build (separate artifact, FW2 / ARMv6)

Pro 903 (Samsung S3C6410 = ARM1176JZF-S, 1200x825 landscape, стилус,
прошивка 2.x) не может запускать бинарь под 633: другой CPU (ARMv6 vs
Cortex-A7), другой glibc (2.5) и другой InkView (нет `DialogSynchro`,
`NetConnectAsync`, `NetInfo`, скана библиотеки через `EVT_STARTSCAN`).
Поэтому для него отдельный контейнер со старым FRSCSDK
(gcc 4.1.2 + glibc 2.5) и отдельный артефакт:

```powershell
podman build -f Containerfile.pocketbook-903 -t pbweb-sdk-903 .
podman run --rm -v ${PWD}:/work -w /work pbweb-sdk-903 sh build-pocketbook-903.sh
# -> target/pbweb-903.app  (copy to /mnt/ext1/applications/pbweb-903.app)
```

Техника: фичи `pbweb-app/device,pro903,keyonly` — Rust-код под
`arm1176jzf-s`, линкер `arm-none-linux-gnueabi-gcc`, `libinkview.so`
подменяется link-time заглушкой (реальная линкуется на устройстве при
загрузке; `DialogSynchro`/`SendEventTo` в заглушке специально нет —
линковка громко упадет, если код их заденет). CPU/линкер задаются в
`build-pocketbook-903.sh`, а не в `.cargo/config.toml`: оба билда делят
триплет `arm-unknown-linux-gnueabi`.

glibc 2.5 против современного Rust std: в `build-pocketbook-903.sh`
вшиты compat-шимы (`accept4` через `accept`+`fcntl`, `getauxval` через
`/proc/self/auxv`, `pthread_setname_np` как no-op, `_Unwind_Backtrace`
как заглушка — бэктрейсы не используются, `panic=abort`). Проверено:
интерпретатор `/lib/ld-linux.so.3`, NEEDED только era-библиотеки
(`libinkview`, `libdl`, `libgcc_s`, `librt`, `libpthread`, `libc`),
FW6-символов в импортах нет (smoke-тест в скрипте), ARMv7-инструкций
(`movw/movt`) в коде нет (единственное срабатывание скана — байты
строки `..._BAC[RUST...]` в пуле литералов, не инструкция).

Отличия 903-сборки (проверено по заголовкам FRSCSDK `inkview.new.h`):

- WiFi: только блокирующий `NetConnect(NULL)` в воркере (+ `silent`,
  если прошивка его экспортирует); состояние — по `QueryNetwork()`,
  `NetInfo` не трогаем (в старых заголовках структура opaque).
- Промпт WiFi — асинхронный `Dialog` + колбэк (блокирующего
  `DialogSynchro` на FW2 нет).
- СТОП/ВЫХОД не шлют `EVT_STARTSCAN` (сервиса сканера на FW2 нет) —
  файлы все равно сразу на диске, Библиотеку обновить вручную.
- Сетевые события `EVT_NET_*` на FW2 не приходят — только поллинг.

## Управление только кнопками (все сборки)

Каждое действие нижней панели доступно с клавиш — маршрутизация в
`pb-ui::key_action` (чистая функция + хост-тесты `cargo test -p pb-ui`):

| Клавиша | Status / Log | Files |
|---|---|---|
| MENU | СТАРТ/СТОП-тоггл (везде) | СТАРТ/СТОП-тоггл |
| OK | СТАРТ/СТОП-тоггл | войти в папку (= тап по строке) |
| LEFT / RIGHT | смена таба | вверх / войти |
| UP / DOWN | переход к Files | движение селекции |
| PREV / NEXT | смена таба | страница вверх/вниз |
| BACK | ВЫХОД | ВВЕРХ; в корне → Status; повторный BACK → ВЫХОД |

Фича `keyonly` (включена в 903-билд) меняет подсказки на кнопочные
формулировки; сам роутинг клавиш работает во всех сборках, включая 633.

Полевая проверка 903 (без стилуса, только кнопки + `pbweb.log`):
Status OK → старт → OK → стоп; Files UP/DOWN + OK + BACK до корня →
Status → BACK выход; PREV/NEXT листают. Прислать
`/mnt/ext1/pbweb.log` — там коды клавиш, шаги WiFi и `query_network=`.
Дополнительно с устройства (для закрытия рисков): `ls /lib/libgcc_s*`
(бинарь линкует `libgcc_s.so.1` динамически) и вывод
`ls /usr/local/lib/libinkview*` (против какой библиотеки линкуемся).

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
countdown) → huge URL + QR code underneath (scan with phone camera to open
the upload page, already paired), + 6-digit PIN on screen for manual entry,
+ steps on screen when idle. All in Russian, all actions are
big touch buttons: СТАРТ/СТОП, middle button named after the tab it
opens (ФАЙЛЫ/ЖУРНАЛ/СТАТУС), ВЫХОД; tapping a file row opens it.

- СТОП really stops the server (socket closed, accept loop unblocked) — WiFi
  stays on, СТАРТ works again instantly.
- Upload progress on the reader screen: file name, percent, speed (МБ/с),
  progress bar; finished uploads stay as "✓ name (size)" until the next one.
- Log tab shows recent HTTP requests served.
- No top status bar; no blocking `NetConnect*` calls — the UI never hangs.
- Partial screen updates (PocketPuzzles pattern): taps, selection moves,
  progress ticks, countdown, wifi/scan messages and BACK navigation repaint
  only their rects via `PartialUpdate`; `FullUpdate` runs on tab switches,
  server transitions and every 10th partial (anti-ghosting).

WiFi details: online = `NetInfo()->connected` only; `NetConnectSilent` then
`NetConnectAsync` + poll; `EVT_NET_CONNECTED` handling; real IP via `getifaddrs`
(wlan* > eth* > any). Touch: `POINTERDOWN`/`TOUCHDOWN` (47/48/49) with x/y from
event params, PocketPuzzles-style (no `GetTouchInfo` indirection; 6.5 lib only
has `GetTouchInfoI`).

Stop with the ВЫХОД button or BACK: fs sync + exit.
New books land as files immediately; Library metadata (title/author/cover)
is indexed by the firmware on its own schedule — open Library to refresh.

Library rescan (0.2.6+): on СТОП and ВЫХОД the app broadcasts `EVT_STARTSCAN`
to the resident `scanner.app` service — exactly what the firmware's own
`PBScanClient::startDeviceScan()` does (`SendEventTo(-3, 0xD7, 0, 0)`).
Completion arrives as `EVT_SCANSTOPPED` ("Библиотека обновлена" on screen);
scan policy (`scanmode`) is only read for the log, never changed.
No `EVT_STOPSCAN` — the firmware answers it "Not Implemented".

Diagnostics: the app appends to `/mnt/ext1/pbweb.log` (startup, key/touch codes,
WiFi steps, panics). If something crashes, send this file — it tells exactly where.

## API

All `/api/*` require the pairing PIN (`?token=` or `X-Auth-Token`), else 401.

- `GET /` , `GET /files` — web UI (SPA: Home + File Manager)
- `GET /api/status` — `{"version","ip","mode","device","wifi","roots"}`
- `GET /api/roots` — `{"internal":true,"sdcard":bool}`
- `GET /api/files?path=int:/books` or `sd:/...` (+ `q`, `sort=name|size|mtime`,
  `order=asc|desc`, `limit` ≤1000, `offset`) → `{items, total, path, offset, limit}`
- `GET /api/download?path=...`
- `GET /api/zip?path=int:/dir` — folder as .zip (stored, 2000 files / 512MB caps)
- `POST /api/mkdir` `{"path","name"}` / `POST /api/rename` `{"path","name"}`
- `POST /api/move` `{"path","dest"}` (same root only)
- `POST /api/delete` `{"path"}` or `{"paths":[...]}` (files + empty folders only)
- `POST /api/upload?path=int:/dir` multipart `files=@...` (512MB cap)
- `POST /api/exists` `{"dir","names":[...]}` → `{"exists":[...]}` (overwrite check)

## Security model (0.3.0+)

- **Pairing PIN, not a password.** Every СТАРТ generates a fresh random 6-digit
  PIN (`/dev/urandom`, std-only) shown on the e-ink screen next to the URL;
  the QR encodes the URL with `?token=` so a scan lands straight in. STOP
  invalidates it. No password to forget, nothing stored.
- **Everything under `/api/` requires the PIN** (`?token=` for links/QR,
  `X-Auth-Token` for fetch) with constant-time compare, otherwise 401.
  Wrong guesses count towards a progressive block (5 free, then 30s doubling
  up to 10 min) — the 6-digit space can't be brute-forced over LAN.
- **Mutations are JSON-only + same-origin checked**: urlencoded fallback
  removed, so plain cross-site form POSTs can't drive the API; a present
  `Origin`/`Referer` must match the server's own `Host`.
- **Confinement**: roots are canonicalized (`canonicalize` + prefix check),
  symlinks are rejected, storage roots can't be renamed/moved/deleted/zipped;
  upload names reject control characters and >255B.
- **Bounded resources**: uploads stream to `.part` files (RAM stays flat),
  512MB/req cap enforced pre-read via `Content-Length`; zips stream from a
  temp file on flash (deleted after the transfer), 512MB/2000 files caps.
- **Fail closed**: OS error details stay in the device log (`/mnt/ext1/pbweb.log`
  + Log tab), responses carry generic messages; server runs only while you
  explicitly started it, plus 15-min idle auto-stop.
- **Deliberately HTTP** (trusted LAN): no TLS theater without a PKI; the PIN
  is shown out-of-band on the e-ink screen, not typed over the network
  except once per pairing inside your own WiFi.

## Notes / limits (v1)

- IPs: shown as placeholder until first route; fill from your router's client list if `192.168.0.?`.
- Port: 8080..8090 auto-fallback.
