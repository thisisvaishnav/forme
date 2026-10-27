#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

mkdir -p dist

echo "==> compiling wasm (release)"
cargo build --release --target wasm32-unknown-unknown

echo "==> binding wasm"
wasm-bindgen --target web --no-typescript \
  --out-dir dist/pkg \
  target/wasm32-unknown-unknown/release/forme.wasm

cp web/index.html web/styles.css dist/

echo "==> built dist/"
ls -la dist dist/pkg
