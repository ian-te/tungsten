#!/usr/bin/env bash
# Builds the Rust core to WebAssembly and inlines it into single-file pages:
#   public/index.html          the site (tungsten.iante.xyz), deployed with wrangler
#   dist/artifact.html         the same page as a claude.ai artifact body
# The site carries a hash of its sources; `./build-web.sh --check` fails when the
# committed page is older than them.
set -euo pipefail
cd "$(dirname "$0")"

stamp() { cat src/*.rs Cargo.toml Cargo.lock web/page.html build-web.sh | sha256sum | cut -c1-16; }
if [[ "${1:-}" == "--check" ]]; then
  want="sources $(stamp)"
  if ! grep -q "$want" public/index.html; then
    echo "public/index.html is out of date: run ./build-web.sh and commit it" >&2
    exit 1
  fi
  echo "public/index.html matches its sources"
  exit 0
fi

cargo build --lib --release --target wasm32-unknown-unknown --no-default-features
mkdir -p public dist
STAMP="$(stamp)" python3 - <<'PY'
import base64, os, pathlib
read = lambda p: base64.b64encode(pathlib.Path(p).read_bytes()).decode()
wasm = read("target/wasm32-unknown-unknown/release/cinestill.wasm")
page = pathlib.Path("web/page.html").read_text()
for marker in ("__WASM_BASE64__", "__STANDALONE__", '<div class="app">'):
    assert marker in page, marker
page = page.replace("__WASM_BASE64__", wasm)

artifact = page.replace("__STANDALONE__", "false")
pathlib.Path("dist/artifact.html").write_text(artifact)

# The site needs its own document skeleton: title, fonts and styles go in <head>.
head, body = page.replace("__STANDALONE__", "true").split('<div class="app">', 1)
icon = ("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E"
        "%3Ccircle cx='16' cy='16' r='14' fill='%23ff5b2e' opacity='.35'/%3E"
        "%3Ccircle cx='16' cy='16' r='7' fill='%23fff3e8'/%3E%3C/svg%3E")
site = (
    '<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n'
    f'<!-- sources {os.environ["STAMP"]} -->\n'
    '<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">\n'
    '<meta name="description" content="Give your photos a film look: CineStill halation, Portra, Ektar, Superia, '
    'Tri-X, grain and light leaks. Runs in your browser; photos never leave your device.">\n'
    f'<link rel="icon" href="{icon}">\n'
    '<style>body{margin:0}img{max-width:100%}</style>\n'
    f'{head.strip()}\n</head>\n<body>\n<div class="app">{body}</body>\n</html>\n'
)
pathlib.Path("public/index.html").write_text(site)
print(f"public/index.html ({len(site) // 1024} KB), dist/artifact.html, wasm {len(wasm) * 3 // 4 // 1024} KB")
PY
