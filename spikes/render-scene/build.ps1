# Build the browser artifact: cargo wasm32 -> wasm-bindgen -> wasm-opt -> web/pkg/.
# Requires wasm-bindgen-cli (0.2.125) and wasm-opt on PATH (here: ~/.local/bin + the rustup bin).
$ErrorActionPreference = "Stop"
$env:Path = "C:\Users\saihi\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin;C:\Users\saihi\.local\bin;$env:Path"
$crate = "metrocalk_render_spike"
$wasm = "target/wasm32-unknown-unknown/release/$crate.wasm"

cargo build --release --target wasm32-unknown-unknown --lib
wasm-bindgen --target web --no-typescript --out-dir web/pkg $wasm
# wasm-bindgen 0.2.x emits reference-types + bulk-memory; wasm-opt must be told to accept them.
wasm-opt -Oz `
  --enable-reference-types --enable-bulk-memory --enable-mutable-globals `
  --enable-nontrapping-float-to-int --enable-sign-ext `
  -o "web/pkg/${crate}_bg.wasm" "web/pkg/${crate}_bg.wasm"
Write-Host "built web/pkg/${crate}_bg.wasm ($([math]::Round((Get-Item "web/pkg/${crate}_bg.wasm").Length/1KB,1)) KB)"
