# vfp-server: one workspace for all your devices

Tracing needs the original file and the secret key. Copying them to every device wastes space and spreads the key.
So the server keeps them. A device sends only the leaked file, and gets the answer.

```
phone / laptop / PC  --(leaked file, token)-->  vfp-server  (master.key, vfp.db, originals)
                     <------(verdict, ranking)--
```

## Start the server

```bash
docker compose -f deploy/docker-compose.yml up -d
docker compose -f deploy/docker-compose.yml logs vfp      # first start prints the admin token. Copy it.
```

Or without Docker: `cargo run --release -p vfp-server -- --data ./data serve`.
The server speaks plain HTTP. For the internet, put an HTTPS proxy (Caddy, nginx, Traefik) in front,
and start the server with `--trust-proxy`. See [SECURITY.md](SECURITY.md).
**Back up the data folder (volume).** Without the key nobody can trace a leak.

## Encryption (Protected mode)

A new server is **uninitialized**. It refuses data until an admin creates a vault:

```bash
vfp connect https://vfp.example.com --token vfp_...
vfp vault init            # asks for a strong passphrase. The server never sees it.
```

After that the server stores originals, `master.key`, receiver and asset names, and marked copies **encrypted**.
Each device unlocks with the passphrase and sends the derived data key with every request
(header `X-VFP-Key`). The server uses it in memory, and never writes it to disk.

- The server **refuses the key header on plain HTTP**. It accepts it only from a trusted HTTPS proxy
  (`--trust-proxy` with `X-Forwarded-Proto: https`), or from the same machine (loopback, for tests).
- The client refuses to send the key to a non-loopback `http://` address.
- Without the key the server answers `423 locked`. With a wrong key it answers `403`.
- `--plain` starts a fresh server with no encryption. Use it only for tests.
- `vfp-server --data ./data migrate` encrypts an existing plain data folder
  (passphrase from `VFP_PASSPHRASE_FILE` or stdin).
- A lost passphrase cannot be recovered.

## Zero-knowledge mode

Create the vault with `vfp vault init --zero-knowledge` (or choose it in the app). The server never gets the
passphrase or key. It stores encrypted blobs in `data/blobs/`. The pipeline routes (`/v1/assets`, `/v1/mark`, `/v1/trace`) return 409.

Blob API (a key header is refused with 400):

- `GET /v1/blobs` lists `{id, size, etag}`. `GET /v1/blobs/<id>` downloads. Any token may read.
- `PUT /v1/blobs/<id>` needs an admin token and one precondition: `If-None-Match: *` (create only) or
  `If-Match: <etag>` (replace that version). No precondition gives 428. A stale one gives 412.
- `PUT /v1/vault` replaces `vault.json` (new passphrase). Admin only.
- Ids: letters, digits, `.`, `_`, `-`, up to 100 characters, no leading dot.

Blobs: `master.enc`, `vfp.db.enc` (the sealed database), `o-<random>.vfpenc` (originals).

## Tokens

| Role | May do |
|---|---|
| `admin` | everything: add receivers and originals, make copies, download copies, trace |
| `trace` | check a leaked file, list receivers and assets |

```bash
vfp-server --data ./data token new --role trace --label phone    # shown once, stored only as a hash
vfp-server --data ./data token list
vfp-server --data ./data token revoke <id>                       # works at once
```

## Use it from a device

```bash
vfp connect https://vfp.example.com --token vfp_...    # saves the address in <home>/server.json
vfp trace leaked.mp4                                   # uploads only leaked.mp4
vfp asset add film.mp4 --name film                     # admin: uploads the original once
vfp mark film --all --out marked                       # the server marks; the copies are downloaded
vfp disconnect                                         # back to this device's own workspace
```

You can also use `--server URL --token T` or the variables `VFP_SERVER` and `VFP_TOKEN`.

## Limits and safety

- `--max-upload-mb` (default 4096): bigger uploads are refused.
- `--jobs` (default 2): heavy jobs that run at the same time. Other requests wait.
- `audit.log` in the data folder: time, token label, action (trace, mark, download, add), ids and counts only. No names.
- Uploaded files are deleted when the request ends.
- `<home>/server.json` on a device holds the token in clear text. Keep that folder private.

## API (all under `/v1`, header `Authorization: Bearer <token>`)

| Call | Role | Body |
|---|---|---|
| `GET /info` | any | |
| `GET /vault`, `POST /vault` | any / admin | `vault.json` (public-safe). `POST` also needs the key header. |
| `GET /receivers`, `POST /receivers` | any / admin | `{"name","note"}` |
| `GET /assets`, `POST /assets?filename=..&name=..&max_receivers=..` | any / admin | file bytes |
| `POST /mark` | admin | `{"asset","receivers":[..]}` |
| `GET /copies?file=..` | admin | |
| `POST /trace?filename=..&asset=..` | any | file bytes |

## Not done yet

- Requests are synchronous: a long video trace keeps the connection open. A job id with progress comes later.
- Uploads are not resumable.
- Docker image not built or tested yet. Server tested with `cargo test -p vfp-server` (real process, real HTTP).
