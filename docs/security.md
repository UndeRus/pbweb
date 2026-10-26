# Security model (0.3.0+)

- **Pairing PIN, not a password.** Every START generates a fresh random 6-digit
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
