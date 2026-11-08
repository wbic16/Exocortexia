//! exo-core — the Exocortexia foundation crate.
//!
//! Everything in here is dependency-light on purpose: the only external
//! crates are `libphext` (the phext format itself) and `sha2` (SHA-256).
//! Higher layers (exo-sso, the demo apps, the load simulator) build on:
//!
//! * [`sha256_hex`], [`now_unix`], [`today_epoch_day`], [`iso_date`]
//! * [`Store`] — a `BTreeMap<String, String>` of coordinate → scroll text,
//!   serialised to / parsed from phext via libphext `explode` / `implode`.
//! * [`AuditLog`] — an append-only, hash-chained audit log stored as
//!   scrolls inside a [`Store`].
//! * Redactions ([`Redaction`], [`load_redactions`], [`add_redaction`],
//!   [`apply_redactions`], [`REDACTION_CATEGORIES`]) modelled on the
//!   withholding categories of Neb. Rev. Stat. § 84-712.05.
//! * [`Rng`] — a deterministic LCG so every seed corpus is reproducible.
//! * [`http`] — a std-only, thread-per-connection mini HTTP server.

pub mod http;

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Hashing and time
// ---------------------------------------------------------------------------

/// Lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

/// Seconds since the Unix epoch (UTC). Returns 0 if the system clock is
/// somehow set before the epoch.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Today as an epoch-day count: `now_unix() / 86400`.
pub fn today_epoch_day() -> i64 {
    now_unix() / 86400
}

/// Convert an epoch-day count to an ISO calendar date (`YYYY-MM-DD`).
///
/// Implements Howard Hinnant's `civil_from_days` algorithm, which is exact
/// for the full proleptic Gregorian calendar, including negative day
/// counts (dates before 1970-01-01).
pub fn iso_date(epoch_day: i64) -> String {
    let z = epoch_day + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // day of era, [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // year of era, [0, 399]
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year, [0, 365]
    let mp = (5 * doy + 2) / 153; // month index, Mar=0 .. Feb=11
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    if month <= 2 {
        year += 1;
    }
    format!("{:04}-{:02}-{:02}", year, month, day)
}

// ---------------------------------------------------------------------------
// Coordinates
// ---------------------------------------------------------------------------

/// True when `s` is a structurally valid phext coordinate of the form
/// `z3.z2.z1/y3.y2.y1/x3.x2.x1` with every component >= 1.
///
/// The shape is checked component-by-component first (libphext's
/// `to_coordinate` falls back to a default coordinate on malformed input
/// rather than erroring), then the parsed coordinate must pass libphext's
/// own `validate_coordinate`, and it must round-trip to the input string
/// (which rejects non-canonical spellings such as `01.1.1/...`).
pub fn valid_coord(s: &str) -> bool {
    let groups: Vec<&str> = s.split('/').collect();
    if groups.len() != 3 {
        return false;
    }
    for group in &groups {
        let parts: Vec<&str> = group.split('.').collect();
        if parts.len() != 3 {
            return false;
        }
        for part in parts {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
            match part.parse::<u64>() {
                Ok(n) if n >= 1 => {}
                _ => return false,
            }
        }
    }
    let coord = libphext::phext::to_coordinate(s);
    coord.validate_coordinate() && coord.to_string() == s
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// A phext-backed scroll store: coordinate string → scroll text.
///
/// In memory this is a sorted map; on disk / on the wire it is a single
/// phext document produced by libphext `implode` and read back with
/// `explode`.
#[derive(Debug, Clone, Default)]
pub struct Store {
    pub map: BTreeMap<String, String>,
}

impl Store {
    pub fn new() -> Store {
        Store {
            map: BTreeMap::new(),
        }
    }

    /// Parse a phext document into a Store. Keys are the canonical
    /// `Coordinate::to_string()` form of each scroll's coordinate.
    pub fn from_phext(text: &str) -> Store {
        let mut store = Store::new();
        if text.is_empty() {
            return store;
        }
        let exploded: HashMap<libphext::phext::Coordinate, String> =
            libphext::phext::explode(text);
        for (coord, scroll) in exploded {
            store.map.insert(coord.to_string(), scroll);
        }
        store
    }

    /// Serialise the store to a phext document.
    pub fn to_phext(&self) -> String {
        if self.map.is_empty() {
            return String::new();
        }
        let mut exploded: HashMap<libphext::phext::Coordinate, String> = HashMap::new();
        for (coord, scroll) in &self.map {
            exploded.insert(libphext::phext::to_coordinate(coord), scroll.clone());
        }
        libphext::phext::implode(exploded)
    }

    pub fn get(&self, coord: &str) -> Option<&str> {
        self.map.get(coord).map(|s| s.as_str())
    }

    pub fn set(&mut self, coord: &str, text: &str) {
        self.map.insert(coord.to_string(), text.to_string());
    }

    pub fn remove(&mut self, coord: &str) -> Option<String> {
        self.map.remove(coord)
    }

    /// All coordinates in the store, sorted ascending.
    pub fn coords(&self) -> Vec<String> {
        self.map.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_phext())
    }

    pub fn load(path: &Path) -> std::io::Result<Store> {
        let text = std::fs::read_to_string(path)?;
        Ok(Store::from_phext(&text))
    }
}

// ---------------------------------------------------------------------------
// Audit log (hash-chained, stored as scrolls in a Store)
// ---------------------------------------------------------------------------

/// Coordinate prefix for audit entries: entry `seq` lives at
/// `1.1.1/1.1.1/2.1.<seq>`.
pub const AUDIT_PREFIX: &str = "1.1.1/1.1.1/2.1.";

/// The `prev_hash` of the first entry in a chain.
pub const AUDIT_GENESIS: &str = "GENESIS";

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub seq: u64,
    pub ts: i64,
    pub actor: String,
    pub action: String,
    pub coord: String,
    pub payload_sha256: String,
    pub prev_hash: String,
    pub hash: String,
}

/// Fields stored in an audit line must not contain `|` (the field
/// separator) or newlines (one entry is one scroll, one line).
fn sanitize_field(s: &str) -> String {
    s.replace('|', "/")
        .replace('\n', " ")
        .replace('\r', " ")
}

fn audit_hash(
    prev_hash: &str,
    seq: u64,
    ts: i64,
    actor: &str,
    action: &str,
    coord: &str,
    payload_sha256: &str,
) -> String {
    sha256_hex(
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            prev_hash, seq, ts, actor, action, coord, payload_sha256
        )
        .as_bytes(),
    )
}

#[derive(Debug, Clone, Default)]
pub struct AuditLog {
    pub entries: Vec<AuditEntry>,
}

impl AuditLog {
    pub fn new() -> AuditLog {
        AuditLog { entries: Vec::new() }
    }

    /// Load the audit chain out of a Store, ordered by sequence number.
    /// Malformed or non-audit scrolls are skipped.
    pub fn open(store: &Store) -> AuditLog {
        let mut found: Vec<(u64, AuditEntry)> = Vec::new();
        for coord in store.coords() {
            let suffix = match coord.strip_prefix(AUDIT_PREFIX) {
                Some(s) => s,
                None => continue,
            };
            let seq_hint: u64 = match suffix.parse() {
                Ok(n) => n,
                Err(_) => continue,
            };
            let line = match store.get(&coord) {
                Some(l) => l,
                None => continue,
            };
            let fields: Vec<&str> = line.trim_end().split('|').collect();
            if fields.len() != 8 {
                continue;
            }
            let seq: u64 = fields[0].parse().unwrap_or(seq_hint);
            let ts: i64 = fields[1].parse().unwrap_or(0);
            found.push((
                seq,
                AuditEntry {
                    seq,
                    ts,
                    actor: fields[2].to_string(),
                    action: fields[3].to_string(),
                    coord: fields[4].to_string(),
                    payload_sha256: fields[5].to_string(),
                    prev_hash: fields[6].to_string(),
                    hash: fields[7].to_string(),
                },
            ));
        }
        found.sort_by_key(|(seq, _)| *seq);
        AuditLog {
            entries: found.into_iter().map(|(_, e)| e).collect(),
        }
    }

    /// Append an entry: hash the payload, chain onto the previous entry,
    /// write the entry scroll into `store`, and return the new entry.
    pub fn append(
        &mut self,
        store: &mut Store,
        actor: &str,
        action: &str,
        coord: &str,
        payload: &str,
    ) -> AuditEntry {
        let actor = sanitize_field(actor);
        let action = sanitize_field(action);
        let coord = sanitize_field(coord);
        let seq = self
            .entries
            .iter()
            .map(|e| e.seq)
            .max()
            .unwrap_or(0)
            + 1;
        let ts = now_unix();
        let payload_sha256 = sha256_hex(payload.as_bytes());
        let prev_hash = self
            .entries
            .iter()
            .max_by_key(|e| e.seq)
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| AUDIT_GENESIS.to_string());
        let hash = audit_hash(
            &prev_hash,
            seq,
            ts,
            &actor,
            &action,
            &coord,
            &payload_sha256,
        );
        let entry = AuditEntry {
            seq,
            ts,
            actor,
            action,
            coord,
            payload_sha256,
            prev_hash,
            hash,
        };
        let line = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            entry.seq,
            entry.ts,
            entry.actor,
            entry.action,
            entry.coord,
            entry.payload_sha256,
            entry.prev_hash,
            entry.hash
        );
        store.set(&format!("{}{}", AUDIT_PREFIX, entry.seq), &line);
        self.entries.push(entry.clone());
        entry
    }

    /// Recompute the whole chain. `Err` names the first bad sequence
    /// number and why it failed.
    pub fn verify(&self) -> Result<(), String> {
        let mut expected_seq: u64 = 1;
        let mut expected_prev = AUDIT_GENESIS.to_string();
        let mut ordered: Vec<&AuditEntry> = self.entries.iter().collect();
        ordered.sort_by_key(|e| e.seq);
        for entry in ordered {
            if entry.seq != expected_seq {
                return Err(format!(
                    "audit chain broken at seq {}: expected seq {} (gap or reordering)",
                    entry.seq, expected_seq
                ));
            }
            if entry.prev_hash != expected_prev {
                return Err(format!(
                    "audit chain broken at seq {}: prev_hash does not match previous entry hash",
                    entry.seq
                ));
            }
            let recomputed = audit_hash(
                &entry.prev_hash,
                entry.seq,
                entry.ts,
                &entry.actor,
                &entry.action,
                &entry.coord,
                &entry.payload_sha256,
            );
            if recomputed != entry.hash {
                return Err(format!(
                    "audit chain broken at seq {}: hash mismatch (entry was modified after logging)",
                    entry.seq
                ));
            }
            expected_prev = entry.hash.clone();
            expected_seq += 1;
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Redactions (Neb. Rev. Stat. § 84-712.05)
// ---------------------------------------------------------------------------

/// Coordinate prefix for redaction records: redaction `n` lives at
/// `1.1.1/1.1.1/3.1.<n>`.
pub const REDACTION_PREFIX: &str = "1.1.1/1.1.1/3.1.";

/// Withholding categories under Neb. Rev. Stat. § 84-712.05, as short
/// labels. A redaction's `category` should be one of these strings.
pub const REDACTION_CATEGORIES: &[&str] = &[
    "84-712.05(1) student personal information",
    "84-712.05(2) medical records",
    "84-712.05(3) personnel records",
    "84-712.05(4) trade secrets / proprietary information",
    "84-712.05(5) law-enforcement investigative records",
    "84-712.05(6) security plans and vulnerability assessments",
    "84-712.05(7) attorney work product / litigation strategy",
    "84-712.05(8) personal contact information",
];

/// One redaction: the byte range `[start, end)` of the scroll at `target`
/// is withheld under `category`, for `reason`.
#[derive(Debug, Clone, PartialEq)]
pub struct Redaction {
    pub target: String,
    pub start: usize,
    pub end: usize,
    pub category: String,
    pub reason: String,
    pub id: String,
}

fn sanitize_value(s: &str) -> String {
    s.replace('\n', " ").replace('\r', " ")
}

/// Load all redactions stored in `store`, ordered by record number.
pub fn load_redactions(store: &Store) -> Vec<Redaction> {
    let mut found: Vec<(u64, Redaction)> = Vec::new();
    for coord in store.coords() {
        let suffix = match coord.strip_prefix(REDACTION_PREFIX) {
            Some(s) => s,
            None => continue,
        };
        let n: u64 = match suffix.parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let text = match store.get(&coord) {
            Some(t) => t,
            None => continue,
        };
        let mut target = String::new();
        let mut start: usize = 0;
        let mut end: usize = 0;
        let mut category = String::new();
        let mut reason = String::new();
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                match key {
                    "target" => target = value.to_string(),
                    "start" => start = value.parse().unwrap_or(0),
                    "end" => end = value.parse().unwrap_or(0),
                    "category" => category = value.to_string(),
                    "reason" => reason = value.to_string(),
                    _ => {}
                }
            }
        }
        found.push((
            n,
            Redaction {
                target,
                start,
                end,
                category,
                reason,
                id: format!("redaction-{}", n),
            },
        ));
    }
    found.sort_by_key(|(n, _)| *n);
    found.into_iter().map(|(_, r)| r).collect()
}

/// Store a redaction as the next numbered redaction scroll.
pub fn add_redaction(store: &mut Store, r: &Redaction) {
    let next: u64 = store
        .coords()
        .iter()
        .filter_map(|c| c.strip_prefix(REDACTION_PREFIX))
        .filter_map(|s| s.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let text = format!(
        "target={}\nstart={}\nend={}\ncategory={}\nreason={}",
        sanitize_value(&r.target),
        r.start,
        r.end,
        sanitize_value(&r.category),
        sanitize_value(&r.reason)
    );
    store.set(&format!("{}{}", REDACTION_PREFIX, next), &text);
}

/// Apply redactions to `text`, returning a new string.
///
/// Each redaction's byte range `[start, end)` is replaced with
/// `[REDACTED {category}]`. Ranges are applied in descending `start`
/// order so earlier offsets stay valid while later text is rewritten.
/// Out-of-range values are clamped; empty or reversed ranges are
/// skipped. The input is never mutated — `text` is only read.
pub fn apply_redactions(text: &str, rs: &[Redaction]) -> String {
    let mut order: Vec<&Redaction> = rs.iter().collect();
    order.sort_by(|a, b| b.start.cmp(&a.start).then(b.end.cmp(&a.end)));
    // Work on bytes: redaction ranges are byte ranges by contract.
    let mut bytes: Vec<u8> = text.as_bytes().to_vec();
    for r in order {
        if r.start >= r.end {
            continue;
        }
        let start = r.start.min(bytes.len());
        let end = r.end.min(bytes.len());
        if start >= end {
            continue;
        }
        let marker = format!("[REDACTED {}]", r.category);
        bytes.splice(start..end, marker.bytes());
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---------------------------------------------------------------------------
// Deterministic RNG (LCG)
// ---------------------------------------------------------------------------

/// A deterministic pseudo-random generator (Knuth MMIX LCG constants).
/// Same seed → same stream, on every platform and every run.
#[derive(Debug, Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    /// A value in `[0, n)`. Returns 0 when `n == 0`.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        self.next() % n
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_value() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn iso_date_known_values() {
        assert_eq!(iso_date(0), "1970-01-01");
        // Verified externally: 20000 * 86400 = 1728000000 s, and
        // `date -d @1728000000 -u` prints 2024-10-04.
        assert_eq!(iso_date(20_000), "2024-10-04");
        assert_eq!(iso_date(-1), "1969-12-31");
        // 2000-01-01 is 946684800 s = epoch day 10957.
        assert_eq!(iso_date(10_957), "2000-01-01");
        // Leap day: 2024-02-29 is epoch day 19782.
        assert_eq!(iso_date(19_782), "2024-02-29");
    }

    #[test]
    fn valid_coord_accepts_and_rejects() {
        assert!(valid_coord("1.1.1/1.1.1/1.1.1"));
        assert!(valid_coord("1.1.1/1.1.1/2.1.17"));
        assert!(valid_coord("10.20.30/4.5.6/7.8.900"));
        assert!(!valid_coord("0.1.1/1.1.1/1.1.1"));
        assert!(!valid_coord("1.1/1.1.1/1.1.1"));
        assert!(!valid_coord("1.1.1/1.1.1"));
        assert!(!valid_coord("a.b.c/d.e.f/g.h.i"));
        assert!(!valid_coord("1.1.1/1.1.1/1.1.1/1.1.1"));
        assert!(!valid_coord(""));
    }

    #[test]
    fn phext_round_trip_identity() {
        // Deterministic pseudo-random store contents: alphabetic words
        // only, so no phext delimiter bytes can appear inside a scroll.
        let words = [
            "bridge", "memo", "public", "record", "meeting", "ledger", "notice", "audit",
        ];
        let mut rng = Rng::new(42);
        let mut store = Store::new();
        for i in 0..40usize {
            let coord = if i % 5 == 4 {
                format!("1.1.1/1.1.{}/1.1.{}", i / 5 + 1, i + 1)
            } else {
                format!("1.1.1/1.1.1/1.1.{}", i + 1)
            };
            let mut text = String::new();
            for w in 0..6 {
                if w > 0 {
                    text.push(' ');
                }
                text.push_str(words[rng.below(words.len() as u64) as usize]);
            }
            store.set(&coord, &text);
        }
        assert_eq!(store.len(), 40);
        let text = store.to_phext();
        let restored = Store::from_phext(&text);
        assert_eq!(restored.len(), store.len());
        assert_eq!(restored.coords(), store.coords());
        for coord in store.coords() {
            assert_eq!(restored.get(&coord), store.get(&coord), "coord {}", coord);
        }
        // Serialising the restored store yields an equivalent document.
        let restored2 = Store::from_phext(&restored.to_phext());
        assert_eq!(restored2.map, store.map);
    }

    #[test]
    fn store_set_get_remove() {
        let mut store = Store::new();
        store.set("1.1.1/1.1.1/1.1.1", "hello");
        assert_eq!(store.get("1.1.1/1.1.1/1.1.1"), Some("hello"));
        assert_eq!(store.remove("1.1.1/1.1.1/1.1.1"), Some("hello".to_string()));
        assert_eq!(store.get("1.1.1/1.1.1/1.1.1"), None);
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn audit_honest_chain_verifies() {
        let mut store = Store::new();
        let mut log = AuditLog::new();
        log.append(&mut store, "clerk", "create", "1.1.1/1.1.1/1.1.1", "payload one");
        log.append(&mut store, "clerk", "update", "1.1.1/1.1.1/1.1.1", "payload two");
        log.append(&mut store, "auditor", "export", "1.1.1/1.1.1/1.1.2", "payload three");
        assert_eq!(log.len(), 3);
        assert_eq!(log.entries[0].seq, 1);
        assert_eq!(log.entries[0].prev_hash, AUDIT_GENESIS);
        log.verify().expect("honest chain must verify");
        // And the chain reloaded from the store alone also verifies.
        let reloaded = AuditLog::open(&store);
        assert_eq!(reloaded.len(), 3);
        reloaded.verify().expect("reloaded chain must verify");
    }

    #[test]
    fn audit_tampered_payload_fails_verification() {
        let mut store = Store::new();
        let mut log = AuditLog::new();
        log.append(&mut store, "clerk", "create", "1.1.1/1.1.1/1.1.1", "payload one");
        log.append(&mut store, "clerk", "update", "1.1.1/1.1.1/1.1.1", "payload two");

        // Negative control 1: flip one payload hash in memory.
        let mut tampered = log.clone();
        let first = tampered.entries[0].payload_sha256.as_bytes()[0];
        let flipped = if first == b'a' { 'b' } else { 'a' };
        tampered.entries[0].payload_sha256.replace_range(0..1, &flipped.to_string());
        let err = tampered
            .verify()
            .expect_err("tampered payload hash must fail verification");
        assert!(err.contains("seq 1"), "error names the bad seq: {}", err);

        // Negative control 2: flip one character of a stored scroll,
        // reload from the store, and verification must fail there too.
        let coord = format!("{}2", AUDIT_PREFIX);
        let line = store.get(&coord).unwrap().to_string();
        let mut bytes = line.into_bytes();
        bytes[3] = if bytes[3] == b'0' { b'9' } else { b'0' };
        store.set(&coord, &String::from_utf8(bytes).unwrap());
        let reloaded = AuditLog::open(&store);
        assert!(reloaded.verify().is_err());
    }

    #[test]
    fn redaction_round_trip_and_apply_never_mutates_input() {
        let mut store = Store::new();
        let original = String::from("Contact Jane Doe at 555-010-2233 about the bridge memo.");
        let start = original.find("555-010-2233").unwrap();
        let r = Redaction {
            target: "1.1.1/1.1.1/1.1.1".to_string(),
            start,
            end: start + "555-010-2233".len(),
            category: REDACTION_CATEGORIES[7].to_string(),
            reason: "phone number in a public record".to_string(),
            id: String::new(),
        };
        add_redaction(&mut store, &r);
        let loaded = load_redactions(&store);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].target, r.target);
        assert_eq!(loaded[0].start, r.start);
        assert_eq!(loaded[0].end, r.end);
        assert_eq!(loaded[0].category, r.category);
        assert_eq!(loaded[0].reason, r.reason);
        assert_eq!(loaded[0].id, "redaction-1");

        let before = original.clone();
        let redacted = apply_redactions(&original, &loaded);
        assert_eq!(original, before, "apply_redactions must not mutate its input");
        assert!(!redacted.contains("555-010-2233"));
        assert!(redacted.contains(&format!("[REDACTED {}]", REDACTION_CATEGORIES[7])));
        assert!(redacted.starts_with("Contact Jane Doe at "));
        assert!(redacted.ends_with(" about the bridge memo."));
    }

    #[test]
    fn redaction_multiple_ranges_descending() {
        let text = "aaa SECRET1 bbb SECRET2 ccc";
        let s1 = text.find("SECRET1").unwrap();
        let s2 = text.find("SECRET2").unwrap();
        let rs = vec![
            Redaction {
                target: "t".into(),
                start: s1,
                end: s1 + 7,
                category: "cat-a".into(),
                reason: "r".into(),
                id: "1".into(),
            },
            Redaction {
                target: "t".into(),
                start: s2,
                end: s2 + 7,
                category: "cat-b".into(),
                reason: "r".into(),
                id: "2".into(),
            },
        ];
        // Passed in ascending order on purpose: apply must reorder.
        assert_eq!(
            apply_redactions(text, &rs),
            "aaa [REDACTED cat-a] bbb [REDACTED cat-b] ccc"
        );
    }

    #[test]
    fn rng_is_deterministic_and_bounded() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next(), b.next());
        }
        let mut c = Rng::new(7);
        for _ in 0..100 {
            assert!(c.below(10) < 10);
        }
        assert_eq!(Rng::new(1).below(0), 0);
    }
}
