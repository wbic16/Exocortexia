//! seedgen: write the standard Exocortexia seed phexts to `seed/out/`.
//!
//! Outputs (paths are anchored to this crate's manifest directory, so the
//! files always land in `seed/out/` regardless of the caller's cwd):
//! - `seed/out/records-100k.phext`   — `records_corpus(100_000)`
//! - `seed/out/minutes.phext`        — `minutes_corpus(500, 10)`
//! - `seed/out/contracts-10k.phext`  — `contracts_corpus(10_000, today)`
//!
//! For each file it prints the byte size and the SHA-256 of the written
//! bytes (via `exo_core::sha256_hex`).

use std::fs;
use std::path::PathBuf;

fn write_corpus(out_dir: &PathBuf, name: &str, store: &exo_core::Store) -> std::io::Result<()> {
    let text = store.to_phext();
    let path = out_dir.join(name);
    fs::write(&path, text.as_bytes())?;
    println!(
        "{} docs={} bytes={} sha256={}",
        path.display(),
        store.len(),
        text.len(),
        exo_core::sha256_hex(text.as_bytes())
    );
    Ok(())
}

fn main() -> std::io::Result<()> {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("out");
    fs::create_dir_all(&out_dir)?;

    let records = exo_seed::records_corpus(100_000);
    write_corpus(&out_dir, "records-100k.phext", &records)?;

    let minutes = exo_seed::minutes_corpus(500, 10);
    write_corpus(&out_dir, "minutes.phext", &minutes)?;

    let contracts = exo_seed::contracts_corpus(10_000, exo_core::today_epoch_day());
    write_corpus(&out_dir, "contracts-10k.phext", &contracts)?;

    Ok(())
}
