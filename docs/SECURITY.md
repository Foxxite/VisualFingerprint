# Security: how your files are protected

The files this tool handles can be sensitive. This page says what is protected, how, and what is not.

## What is encrypted

| Data | Where | How |
|---|---|---|
| Original files | `originals/<random>.vfpenc` | XChaCha20-Poly1305, streaming, 1 MiB chunks. The file name is random. |
| `master.key` | `master.key.enc` | Sealed with a sub-key of the data key. |
| Receiver names and notes, asset names, file names, output paths | Database fields | Each field sealed. Lookups use an HMAC blind index. |
| Marked copies kept on the server | `marked/<blind index>.vfpenc` | Same format. The plain copy is wiped after it is made. |

Ids, counts, sizes and times are not hidden.

## Keys

```
passphrase --Argon2id (256 MiB, t=3)--> key-encrypting key --unwraps--> data key (random, 256 bit)
data key --HKDF--> sub-keys: files, master, db-fields, db-index
```

- `vault.json` holds the wrapped data key and the Argon2 settings. It is safe to back up.
  Someone who has it can try passphrases offline, so the passphrase must be strong.
- Changing the passphrase re-wraps the data key. No file is re-encrypted.
- The data key lives in memory only. The app wipes it on Lock and after 15 minutes without use.
- **The passphrase gate**: at least 64 bits and 12 characters. The score is the lower of a Shannon estimate
  and a pattern-based guess estimate (zxcvbn). The "Make one for me" button makes 7 random words.

## Two places, same format

- **This device**: `vfp init` (or the app setup) makes an encrypted workspace. `vfp vault init` encrypts an old one.
- **Server (Protected mode)**: see [SERVER.md](SERVER.md). The client sends the data key per request over HTTPS.
  The passphrase never leaves the client.

## Temporary files

Marking and tracing audio or video needs a plain file for FFmpeg. It goes in a private folder
(mode 0700 on Unix), is wiped when the job ends, and leftovers are swept at start-up.

## Honest limits

- **Protected mode does not protect against a compromised running server.** The server sees the key while it works.
  A host admin who reads process memory can read your files. (Zero-knowledge mode is planned; it is not built yet.)
- Overwriting a file does not reliably destroy it on an SSD or a journaling file system.
  Use full-disk encryption on every machine that holds plain data, even briefly.
- **A lost passphrase means lost data.** There is no recovery.
- `GET /v1/vault` is open to every valid token (a new device needs it). Offline guessing is limited only by
  passphrase strength and the Argon2 cost.
- The server sees metadata: sizes, counts, times, token ids.
- Built-in TLS is not done. Use an HTTPS proxy and `--trust-proxy`.
- Database fields are bound to their purpose but not to the row id. An attacker with write access to the
  database could swap two sealed values of the same kind. Reading is still impossible.
- Encrypted workspaces cannot be opened by the old Python version.
- Marked copies you give to receivers are not covered. Protect them as you share them.
