# tungsten

Live at https://tungsten.iante.xyz


A tiny Rust app that gives any photo the CineStill 800T look: a red halation
glow around bright lights, a tungsten cast with teal shadows, faded blacks
and film grain. One core (`src/lib.rs`) runs as a command-line tool and,
compiled to WebAssembly, inside a single-file web page.

## Command line

```bash
cargo run --release -- photo.jpg photo-800t.jpg
cargo run --release -- photo.jpg photo-50d.jpg --preset 50d --grain 0.2
cargo run --release -- --help
```

Reads and writes JPEG, PNG and WebP.

## Web page

```bash
./build-web.sh   # → public/index.html (the site) and dist/artifact.html, wasm inlined
npm install
npx wrangler deploy
```

`public/index.html` is committed, so deploying needs no Rust toolchain; rebuild it
after changing `src/lib.rs` or `web/page.html`. The site is a static Worker on the
`tungsten.iante.xyz` custom domain (see `wrangler.jsonc`).

Open, drop or paste a photo. Photos never
leave the browser. The live preview is developed at 1100 px on the long edge.
Saves are full resolution, capped at 16 MP so they fit mobile canvas limits.

## How it works

1. Highlights are pooled into a low-resolution mask and blurred twice, a
   tight core and a wide falloff. The glow is added mostly to the red channel
   in linear light, as when light bounces off the film base with no remjet.
2. Tungsten white balance cools the image.
3. A print curve (S-curve and fade), split toning (teal shadows, warm
   highlights), saturation and vignette.
4. Grain: two octaves of value noise, strongest in the midtones.

Glow radius, grain size and vignette are set relative to the image's short
edge, the highlight mask has the same grid at any size, and each pixel takes
the grain averaged over its area. So developing a smaller copy of a photo
gives the same result as shrinking the developed full-size one, and the
preview matches the export. One thing a smaller copy can't keep: lights only
a pixel or two across at full size are dimmed by the shrinking itself, so
they glow less.
