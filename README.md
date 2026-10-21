# tungsten

Live at https://tungsten.iante.xyz


A tiny Rust app that gives any photo a film look: CineStill 800T's red
halation glow around bright lights and tungsten cast, or Portra, Ektar,
Superia and Tri-X, with faded blacks, film grain and light leaks. One core
(`src/lib.rs`) runs as a command-line tool and, compiled to WebAssembly,
inside a single-file web page.

## Stocks

| Name         | Look                                            |
|--------------|-------------------------------------------------|
| `800t`       | CineStill 800T: tungsten night, strong halation |
| `50d`        | CineStill 50D: daylight, a softer glow          |
| `portra400`  | Portra 400: warm, soft contrast, kind to skin   |
| `ektar100`   | Ektar 100: vivid, contrasty, very fine grain    |
| `superia400` | Superia 400: green-leaning, cool, punchy grain  |
| `trix400`    | Tri-X 400: black and white, gritty grain        |

## Command line

```bash
cargo run --release -- photo.jpg photo-800t.jpg
cargo run --release -- photo.jpg photo-portra.jpg --preset portra400 --leak 0.4
cargo run --release -- *.jpg --out-dir developed --preset trix400
cargo run --release -- --help
```

Reads JPEG, PNG and WebP; writes JPEG, PNG or WebP. JPEGs keep the
original's camera details (turned upright, without its location unless
`--keep-location`).

## Web page

```bash
./build-web.sh          # → public/index.html (the site) and dist/artifact.html, wasm inlined
./build-web.sh --check  # is the committed page built from these sources?
npm install
npx wrangler deploy
```

`public/index.html` is committed, so deploying needs no Rust toolchain; rebuild it
after changing anything in `src/` or `web/page.html` (CI checks). The site is a
static Worker on the `tungsten.iante.xyz` custom domain (see `wrangler.jsonc`).

Every push to `main` deploys it: `.github/workflows/deploy.yml` runs the tests and
`wrangler deploy`, with the repository secret `CLOUDFLARE_API_TOKEN` (a token that can
edit Workers) and, if that token reaches more than one account, `CLOUDFLARE_ACCOUNT_ID`.
It can also be run by hand from the Actions tab. Other branches and pull requests
get tests and lints from `.github/workflows/check.yml`.

Open, drop or paste photos; they never leave the browser.

- The Rust core runs in a background worker, so the page stays responsive while
  saving (on the page itself where workers aren't allowed, as in a claude.ai artifact).
- The live preview is developed at 1100 px on the long edge, with the glow
  worked out from the full-size photo. Double-click it, or press 100%, to see
  the full-size photo developed pixel for pixel, and drag to move around.
- Saves are full size up to 100 MP, encoded a strip at a time, so phones save
  as large as computers. They keep the camera details and, if asked, the
  location; Display P3 photos stay in P3.
- Several photos at once are developed into one ZIP with the same look.
- An optional printed film border: rebate, sprocket holes, edge markings.
- The look is kept in the link (`?stock=portra400&g=0.4`) and in the browser;
  "Copy link to look" shares it.

## How it works

1. Highlights are pooled into a low-resolution mask and blurred twice, a
   tight core and a wide falloff. The glow is added mostly to the red channel
   in linear light, as when light bounces off the film base with no remjet.
   So is a light leak, from the frame's edge.
2. White balance: the tungsten cast, warmth and tint; or black and white,
   through a panchromatic film's response.
3. A print curve (S-curve and fade), split toning (teal shadows, warm
   highlights), saturation and vignette.
4. Grain: two octaves of value noise, strongest in the midtones, with colour
   in the grain only on colour stocks.

Glow radius, grain size, vignette and leaks are set relative to the image's
short edge, the highlight mask has the same grid at any size, and each pixel
takes the grain averaged over its area. So developing a smaller copy of a
photo gives the same result as shrinking the developed full-size one, and
any part of a photo can be developed on its own: the preview, a 100% crop,
or the full photo strip by strip while it's saved (`Developer` in
`src/lib.rs`). One thing a smaller copy can't keep: lights only a pixel or
two across at full size are dimmed by the shrinking itself, so they glow
less; the page avoids that by pooling the glow from the full-size photo.
