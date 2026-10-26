# HTTP API

All `/api/*` require the pairing PIN (`?token=` or `X-Auth-Token`), else 401.
See [security.md](security.md) for the auth/rate-limit details.

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
