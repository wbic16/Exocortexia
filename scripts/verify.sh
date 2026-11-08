#!/usr/bin/env bash
# Exocortexia verification — build, unit tests, seed generation, store
# benchmarks with negative control. HTTP load + SSO smoke steps are in
# docs/VERIFICATION.md (they need running servers).
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== build (release) =="
cargo build --release --workspace

echo "== unit tests =="
cargo test --workspace

echo "== seed generation =="
./target/release/seedgen

echo "== store benchmarks + tamper negative control (100k docs) =="
./target/release/loadsim store --docs 100000

echo "== OK =="
