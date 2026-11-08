# exo-core

The Exocortexia foundation crate. Dependency-light by design: only
`libphext` (the phext format) and `sha2` (SHA-256) beyond std.

## What it provides

- **Hashing & time** — `sha256_hex`, `now_unix`, `today_epoch_day`,
  and `iso_date(epoch_day)` (Howard Hinnant's civil-from-days
  algorithm, exact for negative day counts too).
- **`Store`** — a coordinate → scroll-text map serialised to / parsed
  from phext via libphext `explode` / `implode`, with `save` / `load`
  for single-file persistence, and `valid_coord` for coordinate
  validation.
- **`AuditLog`** — an append-only, hash-chained audit log stored as
  scrolls at `1.1.1/1.1.1/2.1.<seq>` inside a `Store`. Each entry's
  hash covers the previous entry's hash plus the payload's SHA-256;
  `verify()` recomputes the chain and names the first bad sequence.
- **Redactions** — `Redaction` records stored at
  `1.1.1/1.1.1/3.1.<n>`, with `REDACTION_CATEGORIES` listing the
  withholding labels of Neb. Rev. Stat. § 84-712.05.
  `apply_redactions` replaces byte ranges with `[REDACTED <category>]`
  and never mutates its input.
- **`Rng`** — a deterministic LCG (fixed seeds ⇒ reproducible corpora).
- **`exo_core::http`** — a std-only mini HTTP server:
  thread-per-connection, `Request` / `Response`, urlencoded query and
  form parsing, cookies, `esc`, `url_encode` / `url_decode`.

## Tests

`cargo test -p exo-core` — includes the phext round-trip identity
property, audit honest-chain verification plus a tampered-payload
negative control, redaction non-mutation, and known-value checks for
`iso_date` (epoch day 20000 = 2024-10-04) and SHA-256.
