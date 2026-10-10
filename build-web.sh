#!/usr/bin/env bash
# Builds the Rust core to WebAssembly and inlines it into single-file pages:
#   public/index.html          the site (tungsten.iante.xyz), deployed with wrangler
#   dist/artifact.html         the same page as a claude.ai artifact body
set -euo pipefail
cd "$(dirname "$0")"
cargo build --lib --release --target wasm32-unknown-unknown --no-default-features
mkdir -p public dist
python3 - <<'PY'
import base64, pathlib
wasm = pathlib.Path("target/wasm32-unknown-unknown/release/cinestill.wasm").read_bytes()
page = pathlib.Path("web/page.html").read_text()
for marker in ("__WASM_BASE64__", "__STANDALONE__", '<div class="app">'):
    assert marker in page, marker
page = page.replace("__WASM_BASE64__", base64.b64encode(wasm).decode())

artifact = page.replace("__STANDALONE__", "false")
pathlib.Path("dist/artifact.html").write_text(artifact)

# The site needs its own document skeleton: title, fonts and styles go in <head>.
head, body = page.replace("__STANDALONE__", "true").split('<div class="app">', 1)
icon = ("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E"
        "%3Ccircle cx='16' cy='16' r='14' fill='%23ff5b2e' opacity='.35'/%3E"
        "%3Ccircle cx='16' cy='16' r='7' fill='%23fff3e8'/%3E%3C/svg%3E")
site = (
    '<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n'
    '<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">\n'
    '<meta name="description" content="Give your photos a tungsten film look: red halation around lights, '
    'a cool cast, faded blacks and grain. Runs in your browser; photos never leave your device.">\n'
    f'<link rel="icon" href="{icon}">\n'
    '<style>body{margin:0}img{max-width:100%}</style>\n'
    f'{head.strip()}\n</head>\n<body>\n<div class="app">{body}</body>\n</html>\n'
)
pathlib.Path("public/index.html").write_text(site)
print(f"public/index.html ({len(site) // 1024} KB), dist/artifact.html, wasm {len(wasm) // 1024} KB")
PY
