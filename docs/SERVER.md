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
The server speaks plain HTTP. For the internet, put an HTTPS proxy (Caddy, nginx, Traefik) in front.
**Back up the data folder (volume).** Without `master.key` nobody can trace a leak.

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
- `audit.log` in the data folder: time, token label, action (trace, mark, download, add).
- Uploaded files are deleted when the request ends.
- `<home>/server.json` on a device holds the token in clear text. Keep that folder private.

## API (all under `/v1`, header `Authorization: Bearer <token>`)

| Call | Role | Body |
|---|---|---|
| `GET /info` | any | |
| `GET /receivers`, `POST /receivers` | any / admin | `{"name","note"}` |
| `GET /assets`, `POST /assets?filename=..&name=..&max_receivers=..` | any / admin | file bytes |
| `POST /mark` | admin | `{"asset","receivers":[..]}` |
| `GET /copies?file=..` | admin | |
| `POST /trace?filename=..&asset=..` | any | file bytes |

## Not done yet

- Requests are synchronous: a long video trace keeps the connection open. A job id with progress comes later.
- Uploads are not resumable.
- Docker image not built or tested yet. Server tested with `cargo test -p vfp-server` (real process, real HTTP).
