//! Film looks for any photo: CineStill's halation, colour stocks and
//! black-and-white, grain, light leaks.
//!
//! CineStill 800T is Kodak Vision3 motion-picture stock with the remjet
//! backing removed. Without that anti-halation layer, light bounces off the
//! film base and re-exposes the red-sensitive layer, so every bright light
//! gets a red-orange glow. The stock is balanced for tungsten light, which
//! gives daylight scenes a cool cast with teal shadows. The other stocks
//! share the pipeline with their own colour, contrast and grain.
//!
//! The pipeline, per pixel, on 8-bit RGBA:
//! 1. Highlights are pooled into a low-resolution mask ([`MaskBuilder`]) and
//!    blurred twice (a tight core and a wide falloff); that glow is added,
//!    mostly to the red channel, in linear light. So is a light leak.
//! 2. White balance: the tungsten cast, warmth and tint; black and white.
//! 3. An S-curve, faded blacks, split toning (teal shadows, warm highlights),
//!    saturation and a vignette shape the print.
//! 4. Grain: two octaves of value noise, strongest in the midtones.
//!
//! Every size (glow radius, grain cell, vignette, leak) is set relative to the
//! image's short edge, the halation mask has the same grid at any size, and
//! each pixel averages the grain over its area, so developing a smaller copy
//! of a photo gives the same result as shrinking the developed full-size one.
//! That is also what lets a [`Developer`] work on any part of a photo, from
//! a mask pooled at any resolution: a preview, a crop at 100 %, or the full
//! photo strip by strip.

pub mod jpeg;
pub mod meta;
#[cfg(target_arch = "wasm32")]
mod wasm;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Strength of the red glow around highlights. 0–2.
    pub halation: f32,
    /// Glow radius, relative to the image size. 0.25–3.
    pub halation_spread: f32,
    /// Grain amount. 0–1.
    pub grain: f32,
    /// Grain size. 0.5–3.
    pub grain_size: f32,
    /// Tungsten cast: cool balance, teal shadows, warm highlights. 0–1.
    pub tungsten: f32,
    /// White balance, cool to warm. -1–1.
    pub warmth: f32,
    /// Green to magenta. -1–1.
    pub tint: f32,
    /// S-curve contrast. 0–1.
    pub contrast: f32,
    /// Lifted blacks and softened whites. 0–1.
    pub fade: f32,
    /// Colour saturation, 1 = unchanged. 0–2.
    pub saturation: f32,
    /// Black and white, through a panchromatic film's response. 0–1.
    pub mono: f32,
    /// Corner darkening. 0–1.
    pub vignette: f32,
    /// A warm light leak from the film's edge. 0–1.
    pub leak: f32,
    /// Grain pattern and light-leak placement.
    pub seed: u32,
}

impl Params {
    /// Neutral settings: no look at all.
    pub const NONE: Params = Params {
        halation: 0.0,
        halation_spread: 1.0,
        grain: 0.0,
        grain_size: 1.0,
        tungsten: 0.0,
        warmth: 0.0,
        tint: 0.0,
        contrast: 0.0,
        fade: 0.0,
        saturation: 1.0,
        mono: 0.0,
        vignette: 0.0,
        leak: 0.0,
        seed: 0,
    };

    /// CineStill 800T: tungsten night stock. Strong halation, cool cast, visible grain.
    pub const NIGHT_800T: Params = Params {
        halation: 0.8,
        grain: 0.45,
        tungsten: 0.6,
        contrast: 0.45,
        fade: 0.3,
        saturation: 1.05,
        vignette: 0.25,
        seed: 800,
        ..Params::NONE
    };

    /// CineStill 50D: daylight stock. Fine grain, little cast, a softer glow.
    pub const DAYLIGHT_50D: Params = Params {
        halation: 0.4,
        halation_spread: 0.7,
        grain: 0.15,
        grain_size: 0.8,
        tungsten: 0.1,
        contrast: 0.35,
        fade: 0.15,
        saturation: 1.15,
        vignette: 0.15,
        seed: 50,
        ..Params::NONE
    };

    /// Kodak Portra 400: warm, soft contrast, gentle colour, kind to skin.
    pub const PORTRA_400: Params = Params {
        grain: 0.3,
        grain_size: 0.9,
        warmth: 0.5,
        tint: 0.1,
        contrast: 0.12,
        fade: 0.4,
        saturation: 0.85,
        vignette: 0.1,
        seed: 400,
        ..Params::NONE
    };

    /// Kodak Ektar 100: saturated, contrasty and very fine-grained.
    pub const EKTAR_100: Params = Params {
        grain: 0.1,
        grain_size: 0.7,
        warmth: 0.1,
        contrast: 0.6,
        fade: 0.05,
        saturation: 1.4,
        vignette: 0.2,
        seed: 100,
        ..Params::NONE
    };

    /// Fujicolor Superia 400: green-leaning, cool shadows, punchy grain.
    pub const SUPERIA_400: Params = Params {
        grain: 0.4,
        grain_size: 1.1,
        tungsten: 0.3,
        warmth: -0.1,
        tint: -0.45,
        contrast: 0.45,
        fade: 0.15,
        saturation: 1.2,
        vignette: 0.15,
        seed: 401,
        ..Params::NONE
    };

    /// Kodak Tri-X 400: black and white, punchy contrast, gritty grain.
    pub const TRI_X_400: Params = Params {
        grain: 0.6,
        grain_size: 1.3,
        contrast: 0.65,
        fade: 0.1,
        mono: 1.0,
        vignette: 0.2,
        seed: 402,
        ..Params::NONE
    };

    /// Every stock, by its short name.
    pub const STOCKS: [(&'static str, Params); 6] = [
        ("800t", Params::NIGHT_800T),
        ("50d", Params::DAYLIGHT_50D),
        ("portra400", Params::PORTRA_400),
        ("ektar100", Params::EKTAR_100),
        ("superia400", Params::SUPERIA_400),
        ("trix400", Params::TRI_X_400),
    ];
}

impl Default for Params {
    fn default() -> Self {
        Params::NIGHT_800T
    }
}

/// Glow colour in linear light: mostly red, some orange.
const HALATION_RGB: [f32; 3] = [1.0, 0.11, 0.025];
/// Light-leak colour in linear light: orange-red.
const LEAK_RGB: [f32; 3] = [1.0, 0.36, 0.08];

/// Applies the look to `rgba` (row-major, 4 bytes per pixel) in place.
/// Alpha is left untouched.
pub fn apply(rgba: &mut [u8], width: usize, height: usize, p: &Params) {
    let (w, h) = (width, height);
    if w == 0 || h == 0 {
        return;
    }
    assert!(rgba.len() >= w * h * 4, "buffer smaller than width × height × 4");
    let rgba = &mut rgba[..w * h * 4];
    let mask = if p.halation > 0.0 {
        let mut b = MaskBuilder::new(w, h);
        b.add_rows(rgba);
        b.finish()
    } else {
        Mask::default()
    };
    Developer::new(&mask, w, h, p).develop(rgba, 0, 0, w, h);
}

/// Cells on the halation mask's short edge. The grid is fixed relative to the
/// image, so the glow comes out the same at any image size.
const HALO_CELLS: f32 = 256.0;

/// Where a photo's highlights are, pooled into a grid of `HALO_CELLS` on the
/// short edge. It doesn't depend on any setting, so it is worked out once per
/// photo, at full size, and serves every rendering of it.
#[derive(Clone, Debug, Default)]
pub struct Mask {
    data: Vec<f32>,
    w: usize,
    h: usize,
    /// Cells along the photo's short edge.
    cells: f32,
}

/// Pools a photo's highlights into a [`Mask`], a strip of rows at a time,
/// each pixel split by area between the cells it overlaps.
pub struct MaskBuilder {
    w: usize,
    cells: f32,
    mw: usize,
    mh: usize,
    cols: Vec<(usize, f32)>,
    rows: Vec<(usize, f32)>,
    sum: Vec<f32>,
    line: Vec<f32>,
    y: usize,
    to_linear: [f32; 256],
}

impl MaskBuilder {
    pub fn new(w: usize, h: usize) -> Self {
        let short = w.min(h).max(1) as f32;
        // An image smaller than the grid gets one cell per pixel.
        let cells = HALO_CELLS.min(short);
        let scale = cells / short;
        let mw = ((w as f32 * scale).ceil() as usize).max(1);
        let mh = ((h as f32 * scale).ceil() as usize).max(1);
        MaskBuilder {
            w,
            cells,
            mw,
            mh,
            cols: area_split(w, scale, mw),
            rows: area_split(h, scale, mh),
            sum: vec![0.0; mw * mh],
            line: vec![0.0; mw],
            y: 0,
            to_linear: linear_table(),
        }
    }

    /// Adds the next whole rows of the photo, `w * 4` bytes each, top to bottom.
    pub fn add_rows(&mut self, rgba: &[u8]) {
        let (mw, mh) = (self.mw, self.mh);
        for row in rgba.chunks_exact(self.w * 4) {
            let Some(&(r0, wr)) = self.rows.get(self.y) else { return };
            self.y += 1;
            self.line.fill(0.0);
            for (px, &(c0, wc)) in row.as_chunks::<4>().0.iter().zip(&self.cols) {
                let lin = &self.to_linear;
                let (r, g, b) = (lin[px[0] as usize], lin[px[1] as usize], lin[px[2] as usize]);
                // Mostly the brightest channel, so coloured lights glow too,
                // but a near-white source glows more than a lit window.
                let v = 0.65 * r.max(g).max(b) + 0.35 * (r + g + b) / 3.0;
                let e = smoothstep(0.6, 1.0, v);
                let e = e * e;
                self.line[c0] += e * wc;
                self.line[(c0 + 1).min(mw - 1)] += e * (1.0 - wc);
            }
            let r1 = (r0 + 1).min(mh - 1);
            for (mx, &e) in self.line.iter().enumerate() {
                self.sum[r0 * mw + mx] += e * wr;
                self.sum[r1 * mw + mx] += e * (1.0 - wr);
            }
        }
    }

    pub fn finish(mut self) -> Mask {
        // Each cell holds an area-weighted sum; divide by the area it covers.
        let coverage = |split: &[(usize, f32)], n: usize| {
            let mut a = vec![0.0f32; n];
            for &(i, wi) in split {
                a[i] += wi;
                a[(i + 1).min(n - 1)] += 1.0 - wi;
            }
            a
        };
        let (ax, ay) = (coverage(&self.cols, self.mw), coverage(&self.rows, self.mh));
        for (my, cells_row) in self.sum.chunks_exact_mut(self.mw).enumerate() {
            for (c, a) in cells_row.iter_mut().zip(&ax) {
                *c /= (a * ay[my]).max(1e-6);
            }
        }
        Mask { data: self.sum, w: self.mw, h: self.mh, cells: self.cells }
    }
}

/// Develops a photo of `w`×`h` pixels, or any part of it, with one set of
/// [`Params`]. The photo can be any size: the mask may come from a larger
/// copy of it, and parts can be developed in any order.
pub struct Developer {
    w: usize,
    h: usize,
    halo: HaloMap,
    strength: f32,
    to_linear: [f32; 256],
    wb: [f32; 3],
    print: Vec<f32>,
    t: f32,
    sat: f32,
    mono: f32,
    grain_amp: f32,
    vig: f32,
    fine: Octave,
    coarse: Octave,
    leak: Leak,
    /// The columns the last [`Developer::develop`] call covered, and what
    /// depends on them only.
    region: Option<(usize, usize)>,
    col_d2: Vec<f32>,
    halo_cols: Vec<(usize, usize, f32)>,
    halo_row: Vec<f32>,
}

impl Developer {
    pub fn new(mask: &Mask, w: usize, h: usize, p: &Params) -> Self {
        let short = w.min(h).max(1) as f32;
        let halo = HaloMap::new(mask, short, p);

        // White balance, in linear light: the tungsten cast cools, then warmth and tint.
        let t = p.tungsten.clamp(0.0, 1.0);
        let warm = p.warmth.clamp(-1.0, 1.0);
        let tint = p.tint.clamp(-1.0, 1.0);
        let wb = [
            (1.0 - 0.12 * t) * (1.0 + 0.2 * warm),
            (1.0 - 0.02 * t) * (1.0 - 0.12 * tint),
            (1.0 + 0.1 * t) * (1.0 - 0.2 * warm),
        ];
        // Print curve, from linear light to display: sRGB encode, S-curve, fade.
        let k = p.contrast.clamp(0.0, 1.0) * 0.55;
        let fade_lo = p.fade.clamp(0.0, 1.0) * 0.075;
        let fade_hi = p.fade.clamp(0.0, 1.0) * 0.035;
        let print = (0..=LUT_MAX)
            .map(|i| {
                let v = linear_to_srgb(i as f32 / LUT_MAX as f32);
                let v = v - k * (std::f32::consts::TAU * v).sin() / std::f32::consts::TAU;
                fade_lo + v * (1.0 - fade_lo - fade_hi)
            })
            .collect();

        // Grain cell size in pixels; roughly 1px on a 1500px-short-edge image.
        // Each pixel averages the grain over its area, so smaller images get finer,
        // fainter grain per pixel, as if the full-size result were downscaled.
        let cell = short as f64 / 1500.0 * p.grain_size.clamp(0.25, 4.0) as f64;

        Developer {
            w,
            h,
            halo,
            strength: p.halation.max(0.0) * 2.4,
            to_linear: linear_table(),
            wb,
            print,
            t,
            sat: p.saturation.clamp(0.0, 3.0),
            mono: p.mono.clamp(0.0, 1.0),
            grain_amp: p.grain.clamp(0.0, 1.0) * 0.16,
            vig: p.vignette.clamp(0.0, 1.0) * 0.5,
            fine: Octave::new(cell, [0.0, 0.0], p.seed),
            coarse: Octave::new(cell * 2.0, [17.3, 5.1], p.seed ^ 0x9e37_79b9),
            leak: Leak::new(p.leak.clamp(0.0, 1.0), p.seed, w, h),
            region: None,
            col_d2: Vec::new(),
            halo_cols: Vec::new(),
            halo_row: Vec::new(),
        }
    }

    fn set_region(&mut self, x0: usize, rw: usize) {
        if self.region == Some((x0, rw)) {
            return;
        }
        self.region = Some((x0, rw));
        let (cx, cy) = (self.w as f32 * 0.5, self.h as f32 * 0.5);
        let inv_d2 = 1.0 / (cx * cx + cy * cy);
        self.col_d2 = (x0..x0 + rw).map(|x| (x as f32 + 0.5 - cx).powi(2) * inv_d2).collect();
        self.halo_cols = (x0..x0 + rw).map(|x| self.halo.col(x)).collect();
        self.fine.set_cols(x0, rw);
        self.coarse.set_cols(x0, rw);
        self.leak.set_cols(x0, rw);
    }

    /// Develops the `rw`×`rh` pixels at (`x0`, `y0`) of the photo, given in
    /// `rgba` (row-major, 4 bytes per pixel), in place. Alpha is left untouched.
    pub fn develop(&mut self, rgba: &mut [u8], x0: usize, y0: usize, rw: usize, rh: usize) {
        if rw == 0 || rh == 0 {
            return;
        }
        assert!(rgba.len() >= rw * rh * 4, "buffer smaller than width × height × 4");
        self.set_region(x0, rw);
        let (cx, cy) = (self.w as f32 * 0.5, self.h as f32 * 0.5);
        let inv_d2 = 1.0 / (cx * cx + cy * cy);
        let (t, sat, mono, vig, grain_amp) = (self.t, self.sat, self.mono, self.vig, self.grain_amp);
        let has_glow = self.strength > 0.0 && !self.halo.data.is_empty();

        for (dy, row) in rgba.chunks_exact_mut(rw * 4).take(rh).enumerate() {
            let y = y0 + dy;
            let row_d2 = (y as f32 + 0.5 - cy).powi(2) * inv_d2;
            if grain_amp > 0.0 {
                self.fine.start_row(y);
                self.coarse.start_row(y);
            }
            if has_glow {
                self.halo.fill_row(y, &mut self.halo_row);
            }
            let leak_row = self.leak.row(y);
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let glow = if has_glow {
                    let (x0, x1, f) = self.halo_cols[x];
                    let row = &self.halo_row;
                    (row[x0] + (row[x1] - row[x0]) * f) * self.strength
                } else {
                    0.0
                };
                let leak = self.leak.at(x, leak_row);

                let mut lin = [0.0f32; 3];
                for i in 0..3 {
                    lin[i] = (self.to_linear[px[i] as usize] + HALATION_RGB[i] * glow + LEAK_RGB[i] * leak) * self.wb[i];
                }
                if mono > 0.0 {
                    // Panchromatic black and white: a little less blue than the eye.
                    let bw = 0.30 * lin[0] + 0.59 * lin[1] + 0.11 * lin[2];
                    for v in lin.iter_mut() {
                        *v += (bw - *v) * mono;
                    }
                }
                let mut c = [0.0f32; 3];
                for i in 0..3 {
                    c[i] = self.print[(soft_clip(lin[i]) * LUT_MAX as f32) as usize];
                }

                // Split tone: teal shadows, warm highlights; greens lean cyan.
                if t > 0.0 {
                    let lum = luma(c);
                    let sh = (1.0 - lum) * (1.0 - lum) * t;
                    let hi = lum * lum * t;
                    c[0] += -0.045 * sh + 0.035 * hi;
                    c[1] += 0.012 * sh + 0.004 * hi;
                    c[2] += 0.04 * sh - 0.04 * hi;
                    let green_excess = (c[1] - c[0].max(c[2])).max(0.0);
                    c[2] += 0.25 * green_excess * t;
                }

                let lum = luma(c);
                let mut m = 1.0;
                if vig > 0.0 {
                    let fall = smoothstep(0.12, 1.1, self.col_d2[x] + row_d2);
                    m = 1.0 - vig * fall * fall;
                }
                for v in c.iter_mut() {
                    *v = (lum + (*v - lum) * sat) * m;
                }

                if grain_amp > 0.0 {
                    let [mono_fine, red, blue] = self.fine.sample(x);
                    let [mono_coarse, _, _] = self.coarse.sample(x);
                    let g = 0.75 * mono_fine + 0.45 * mono_coarse;
                    let lum = luma(c).clamp(0.0, 1.0);
                    // Film grain shows most in the midtones and least in deep blacks.
                    let amp = grain_amp * (0.35 + 2.6 * lum * (1.0 - lum)) * (0.4 + 0.6 * lum.sqrt());
                    // Colour film's dye clouds differ per layer; black and white has one.
                    let chroma = 0.35 * (1.0 - mono);
                    c[0] += amp * (g + chroma * red);
                    c[1] += amp * g;
                    c[2] += amp * (g + chroma * blue);
                }

                for i in 0..3 {
                    px[i] = (c[i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        }
    }
}

const LUT_MAX: usize = 4095;

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Identity below 0.9, then an exponential shoulder that never reaches 1.
fn soft_clip(v: f32) -> f32 {
    if v <= 0.9 {
        v.max(0.0)
    } else {
        1.0 - 0.1 * (-(v - 0.9) / 0.1).exp()
    }
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The blurred halation glow on the mask's grid, sampled bilinearly for the
/// photo being developed, one row at a time.
struct HaloMap {
    data: Vec<f32>,
    w: usize,
    h: usize,
    /// Mask cells per pixel of the photo being developed.
    scale: f32,
}

impl HaloMap {
    /// Blurs `mask` into a tight core and a wide falloff, with radii set
    /// relative to the short edge.
    fn new(mask: &Mask, short: f32, p: &Params) -> Self {
        if p.halation <= 0.0 || mask.data.is_empty() {
            return HaloMap { data: Vec::new(), w: 0, h: 0, scale: 1.0 };
        }
        let (mw, mh, cells) = (mask.w, mask.h, mask.cells);
        let spread = p.halation_spread.clamp(0.1, 4.0);
        let mut core = mask.data.clone();
        let mut wide = mask.data.clone();
        gaussian(&mut core, mw, mh, cells * 0.0075 * spread);
        gaussian(&mut wide, mw, mh, cells * 0.03 * spread);
        for (c, wide) in core.iter_mut().zip(&wide) {
            *c = 0.55 * *c + 1.6 * wide;
        }
        HaloMap { data: core, w: mw, h: mh, scale: cells / short }
    }

    fn cell(&self, i: usize) -> (usize, usize, f32) {
        let f = ((i as f32 + 0.5) * self.scale - 0.5).max(0.0);
        let i0 = f as usize;
        (i0, i0 + 1, f - i0 as f32)
    }

    /// Left cell, right cell and the weight of the right one, for column `x`.
    fn col(&self, x: usize) -> (usize, usize, f32) {
        let (x0, x1, t) = self.cell(x);
        let last = self.w.max(1) - 1;
        (x0.min(last), x1.min(last), t)
    }

    /// Interpolates the map vertically for image row `y` into `out`.
    fn fill_row(&self, y: usize, out: &mut Vec<f32>) {
        let (y0, y1, t) = self.cell(y);
        let (y0, y1) = (y0.min(self.h - 1), y1.min(self.h - 1));
        let (a, b) = (&self.data[y0 * self.w..][..self.w], &self.data[y1 * self.w..][..self.w]);
        out.clear();
        out.extend(a.iter().zip(b).map(|(a, b)| a + (b - a) * t));
    }
}

/// A light leak: two soft orange blobs bleeding in from one side of the
/// frame, placed by the seed, in coordinates relative to the short edge.
struct Leak {
    amount: f32,
    /// Per blob: centre and radii, in pixels.
    blobs: [([f32; 2], [f32; 2], f32); 2],
    /// Per blob, for the current columns: the horizontal falloff.
    cols: [Vec<f32>; 2],
}

impl Leak {
    fn new(amount: f32, seed: u32, w: usize, h: usize) -> Self {
        let (w, h) = (w as f32, h as f32);
        let short = w.min(h);
        let r = |i: i32| (hash(i, 7, seed ^ 0x51ed_2701) & 0xffff) as f32 / 65535.0;
        // Along the long edge's ends, as when a camera back lets light in.
        let landscape = w >= h;
        let side = if r(0) < 0.5 { 0.0 } else { 1.0 };
        let along = 0.2 + 0.6 * r(1);
        let at = |side: f32, along: f32| if landscape { [side * w, along * h] } else { [along * w, side * h] };
        let big = (0.35 + 0.2 * r(2)) * short;
        let small = (0.15 + 0.1 * r(3)) * short;
        let stretch = |a: f32, b: f32| if landscape { [a, b] } else { [b, a] };
        Leak {
            amount: amount * 1.4,
            blobs: [
                (at(side, along), stretch(big * 0.6, big), 1.0),
                (at(side, (along + 0.25 + 0.2 * r(4)).fract()), stretch(small * 0.5, small), 0.6),
            ],
            cols: [Vec::new(), Vec::new()],
        }
    }

    fn set_cols(&mut self, x0: usize, rw: usize) {
        if self.amount <= 0.0 {
            return;
        }
        for (cols, (c, r, _)) in self.cols.iter_mut().zip(&self.blobs) {
            *cols = (x0..x0 + rw).map(|x| (-((x as f32 + 0.5 - c[0]) / r[0]).powi(2)).exp()).collect();
        }
    }

    /// The vertical falloff of each blob for row `y`, times its strength.
    fn row(&self, y: usize) -> [f32; 2] {
        if self.amount <= 0.0 {
            return [0.0; 2];
        }
        self.blobs.map(|(c, r, k)| k * self.amount * (-((y as f32 + 0.5 - c[1]) / r[1]).powi(2)).exp())
    }

    fn at(&self, x: usize, row: [f32; 2]) -> f32 {
        if self.amount <= 0.0 {
            return 0.0;
        }
        row[0] * self.cols[0][x] + row[1] * self.cols[1][x]
    }
}

fn linear_table() -> [f32; 256] {
    std::array::from_fn(|v| srgb_to_linear(v as f32 / 255.0))
}

/// For each of `len` pixels, the map cell its left edge falls in and the share
/// of the pixel in that cell; the rest goes to the next cell. `scale` (cells
/// per pixel) is at most 1, so a pixel spans at most two cells.
fn area_split(len: usize, scale: f32, cells: usize) -> Vec<(usize, f32)> {
    (0..len)
        .map(|i| {
            let (a, b) = (i as f64 * scale as f64, (i + 1) as f64 * scale as f64);
            let c = (a.floor() as usize).min(cells - 1);
            let share = if b <= (c + 1) as f64 { 1.0 } else { ((c + 1) as f64 - a) / (b - a) };
            (c, share as f32)
        })
        .collect()
}

/// Gaussian blur from three box passes each way, with box widths chosen so
/// the passes add up to `sigma` exactly (Kovesi's mixed widths), not rounded.
fn gaussian(buf: &mut [f32], w: usize, h: usize, sigma: f32) {
    if sigma <= 0.0 {
        return;
    }
    let var12 = 12.0 * sigma * sigma;
    let mut lo = ((var12 / 3.0 + 1.0).sqrt().floor() as usize).max(1);
    if lo.is_multiple_of(2) {
        lo -= 1;
    }
    let lo_f = lo as f32;
    let m = ((var12 - 3.0 * lo_f * lo_f - 12.0 * lo_f - 9.0) / (-4.0 * lo_f - 4.0)).round().clamp(0.0, 3.0) as usize;
    let mut tmp = vec![0.0f32; buf.len()];
    for pass in 0..3 {
        let r = if pass < m { lo / 2 } else { lo / 2 + 1 };
        if r == 0 {
            continue;
        }
        box_blur(buf, &mut tmp, w, h, r, true);
        box_blur(&tmp, buf, w, h, r, false);
    }
}

/// One box pass from `src` into `dst`, along rows or columns, clamping at edges.
fn box_blur(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let inv = 1.0 / (2 * r + 1) as f32;
    let last = len as isize - 1;
    let clamp = |i: isize| i.clamp(0, last) as usize;
    for line in 0..lines {
        let mut acc = 0.0;
        for i in -(r as isize)..=(r as isize) {
            acc += src[at(line, clamp(i))];
        }
        for i in 0..len {
            dst[at(line, i)] = acc * inv;
            let add = clamp(i as isize + r as isize + 1);
            let sub = clamp(i as isize - r as isize);
            acc += src[at(line, add)] - src[at(line, sub)];
        }
    }
}

fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h
}

/// Three 10-bit values in [-1, 1] from one hash.
fn split3(h: u32) -> [f32; 3] {
    const S: f32 = 2.0 / 1023.0;
    [(h & 0x3ff) as f32 * S - 1.0, ((h >> 11) & 0x3ff) as f32 * S - 1.0, (h >> 22) as f32 * S - 1.0]
}

/// Most lattice points one pixel's footprint touches along an axis.
const TAPS: usize = 6;

/// The lattice points a pixel's footprint touches along one axis, from
/// `start`, and the weight of each.
struct Footprint {
    start: i32,
    weights: [f32; TAPS],
    n: usize,
}

/// One octave of value noise (three independent channels in [-1, 1] on a
/// lattice of `cell` pixels), averaged over each pixel's area rather than
/// sampled at a point, so it doesn't alias when a cell is smaller than a pixel.
/// `set_cols` picks the columns, then rows are worked out with `start_row`
/// and `sample`; going down row by row reuses the lattice rows.
struct Octave {
    /// Lattice cells per pixel.
    per_px: f64,
    offset: [f64; 2],
    seed: u32,
    /// Footprint of each image column.
    cols: Vec<Footprint>,
    /// Lattice rows the current image row touches, each from the first
    /// lattice column `cols` touch to the last.
    rows: std::collections::VecDeque<(i32, Vec<[f32; 3]>)>,
    /// Those rows, averaged over the current image row's height.
    line: Vec<[f32; 3]>,
}

impl Octave {
    /// Beyond this many cells per pixel, the noise is drawn per pixel instead.
    const MAX_PER_PX: f64 = 4.0;

    fn new(cell: f64, offset: [f64; 2], seed: u32) -> Self {
        Octave {
            per_px: 1.0 / cell.max(1e-6),
            offset,
            seed,
            cols: Vec::new(),
            rows: Default::default(),
            line: Vec::new(),
        }
    }

    fn set_cols(&mut self, x0: usize, rw: usize) {
        self.cols = (x0..x0 + rw).map(|x| self.footprint(x, 0)).collect();
        self.rows.clear();
    }

    /// Weights of the lattice points over pixel `i` along `axis` (0 for x):
    /// the smoothstep interpolation kernel integrated across the pixel,
    /// divided by its width.
    fn footprint(&self, i: usize, axis: usize) -> Footprint {
        let s = self.per_px;
        if s > Self::MAX_PER_PX {
            // So many cells per pixel that neighbouring pixels are nearly
            // independent: one value per pixel, with the variance an average
            // over `s` cells would have (∫kernel² = 26/35 per cell).
            let mut weights = [0.0; TAPS];
            weights[0] = (26.0 / 35.0 / s).sqrt() as f32;
            return Footprint { start: i as i32, weights, n: 1 };
        }
        let (a, b) = (i as f64 * s + self.offset[axis], (i + 1) as f64 * s + self.offset[axis]);
        let start = a.floor();
        let n = ((b.ceil() - start) as usize + 1).min(TAPS);
        let mut weights = [0.0; TAPS];
        for (k, wk) in weights.iter_mut().enumerate().take(n) {
            let l = start + k as f64;
            *wk = ((kernel_integral(b - l) - kernel_integral(a - l)) / (b - a)) as f32;
        }
        Footprint { start: start as i32, weights, n }
    }

    fn start_row(&mut self, y: usize) {
        let (Some(first), Some(last)) = (self.cols.first(), self.cols.last()) else { return };
        let span = first.start..last.start + last.n as i32;
        let row = self.footprint(y, 1);
        // Rows already hashed are kept while image rows still touch them;
        // going back up starts afresh.
        if self.rows.front().is_some_and(|&(j, _)| j > row.start) {
            self.rows.clear();
        }
        while self.rows.front().is_some_and(|&(j, _)| j < row.start) {
            self.rows.pop_front();
        }
        for j in row.start..row.start + row.n as i32 {
            if self.rows.back().is_some_and(|&(k, _)| k >= j) {
                continue;
            }
            let values = span.clone().map(|i| split3(hash(i, j, self.seed))).collect();
            self.rows.push_back((j, values));
        }
        self.line.clear();
        self.line.resize(span.len(), [0.0; 3]);
        for (wj, (_, values)) in row.weights[..row.n].iter().zip(&self.rows) {
            for (l, v) in self.line.iter_mut().zip(values) {
                for c in 0..3 {
                    l[c] += wj * v[c];
                }
            }
        }
    }

    /// The noise over column `x` of the current columns, in the current row.
    fn sample(&self, x: usize) -> [f32; 3] {
        let col = &self.cols[x];
        let at = &self.line[(col.start - self.cols[0].start) as usize..][..col.n];
        let mut out = [0.0f32; 3];
        for (wi, v) in col.weights[..col.n].iter().zip(at) {
            for c in 0..3 {
                out[c] += wi * v[c];
            }
        }
        out
    }
}

/// ∫ from -1 to `d` of the value-noise kernel, 1 - smoothstep(|t|) on [-1, 1].
fn kernel_integral(d: f64) -> f64 {
    // ∫₀ᵗ smoothstep = t³ - t⁴/2
    let s = |t: f64| t * t * t - 0.5 * t * t * t * t;
    if d <= -1.0 {
        0.0
    } else if d < 0.0 {
        s(1.0 + d)
    } else if d < 1.0 {
        1.0 - s(1.0 - d)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, v: u8) -> Vec<u8> {
        let mut buf = vec![v; w * h * 4];
        for px in buf.chunks_mut(4) {
            px[3] = 255;
        }
        buf
    }

    #[test]
    fn tiny_and_empty_images() {
        apply(&mut [], 0, 0, &Params::default());
        let mut one = solid(1, 1, 128);
        apply(&mut one, 1, 1, &Params::default());
        assert_eq!(one[3], 255);
    }

    #[test]
    fn bright_light_glows_red_around_it() {
        let (w, h) = (200, 200);
        let mut buf = solid(w, h, 10);
        for y in 95..105 {
            for x in 95..105 {
                let i = (y * w + x) * 4;
                buf[i..i + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let p = Params { grain: 0.0, vignette: 0.0, ..Params::default() };
        apply(&mut buf, w, h, &p);
        let i = (100 * w + 112) * 4;
        let (r, g, b) = (buf[i], buf[i + 1], buf[i + 2]);
        assert!(r > g + 20 && r > b + 20, "expected a red halo, got {r},{g},{b}");
        let far = (5 * w + 5) * 4;
        assert!(buf[far] < r, "halo should fade with distance");
    }

    /// Averages `k`×`k` blocks, as resizing a photo down does.
    fn shrink(buf: &[u8], w: usize, h: usize, k: usize) -> Vec<u8> {
        let (sw, sh) = (w / k, h / k);
        let mut out = vec![255; sw * sh * 4];
        for y in 0..sh {
            for x in 0..sw {
                for c in 0..3 {
                    let sum: u32 = (0..k * k).map(|i| buf[((y * k + i / k) * w + x * k + i % k) * 4 + c] as u32).sum();
                    out[(y * sw + x) * 4 + c] = ((sum as f32 / (k * k) as f32) + 0.5) as u8;
                }
            }
        }
        out
    }

    #[test]
    fn a_smaller_copy_develops_the_same() {
        // A dark scene with a grey wall and a lamp, at 4× the size and at 1×.
        let (w, h, k) = (120, 80, 4);
        let mut big = solid(w * k, h * k, 12);
        for y in 0..h * k {
            for x in 0..w * k {
                let lamp = (40 * k..52 * k).contains(&x) && (30 * k..42 * k).contains(&y);
                let wall = x > 70 * k && y > 20 * k;
                if lamp || wall {
                    let i = (y * w * k + x) * 4;
                    big[i..i + 3].fill(if lamp { 255 } else { 120 });
                }
            }
        }
        let mut small = shrink(&big, w * k, h * k, k);
        let p = Params::default();
        apply(&mut big, w * k, h * k, &p);
        apply(&mut small, w, h, &p);
        let big = shrink(&big, w * k, h * k, k);
        let sq: f32 = big
            .as_chunks::<4>()
            .0
            .iter()
            .zip(small.as_chunks::<4>().0)
            .flat_map(|(a, b)| (0..3).map(move |c| (a[c] as f32 - b[c] as f32).powi(2)))
            .sum();
        let rmse = (sq / (w * h * 3) as f32).sqrt();
        // The print curve alone, at the lamp's edges, accounts for about 0.5.
        assert!(rmse < 1.0, "developing a smaller copy differs by {rmse:.2} levels RMS");
    }

    #[test]
    fn tiles_and_strips_develop_like_the_whole() {
        let (w, h) = (90, 70);
        let mut img: Vec<u8> = (0..w * h * 4).map(|i| ((i * 37) % 251) as u8).collect();
        for y in 30..40 {
            img[(y * w + 40) * 4..(y * w + 50) * 4].fill(255);
        }
        let p = Params { leak: 0.6, ..Params::default() };
        let mut whole = img.clone();
        apply(&mut whole, w, h, &p);

        let mut b = MaskBuilder::new(w, h);
        for strip in img.chunks(w * 4 * 13) {
            b.add_rows(strip);
        }
        let mut dev = Developer::new(&b.finish(), w, h, &p);
        // Right to left, bottom to top, in uneven tiles.
        for (x0, rw) in [(60, 30), (0, 60)] {
            for (y0, rh) in [(45, 25), (0, 45)] {
                let mut tile: Vec<u8> = (y0..y0 + rh).flat_map(|y| img[(y * w + x0) * 4..][..rw * 4].to_vec()).collect();
                dev.develop(&mut tile, x0, y0, rw, rh);
                for (r, y) in (y0..y0 + rh).enumerate() {
                    assert_eq!(&tile[r * rw * 4..][..rw * 4], &whole[(y * w + x0) * 4..][..rw * 4], "row {y}");
                }
            }
        }
    }

    #[test]
    fn black_and_white_has_no_colour() {
        let mut buf: Vec<u8> = (0..40 * 30 * 4).map(|i| ((i * 53) % 256) as u8).collect();
        apply(&mut buf, 40, 30, &Params::TRI_X_400);
        assert!(buf.chunks(4).all(|px| px[0] == px[1] && px[1] == px[2]));
    }

    #[test]
    fn a_light_leak_warms_one_side() {
        let (w, h) = (120, 80);
        let mut buf = solid(w, h, 60);
        let p = Params { leak: 1.0, ..Params::NONE };
        apply(&mut buf, w, h, &p);
        let red = |x: usize| buf[(40 * w + x) * 4];
        let (left, right) = (red(2), red(w - 3));
        assert!(left.max(right) > 120 && left.min(right) < 80, "leak at the edges: {left}, {right}");
    }

    #[test]
    fn same_seed_same_grain() {
        let mut a = solid(64, 48, 120);
        let mut b = a.clone();
        apply(&mut a, 64, 48, &Params::default());
        apply(&mut b, 64, 48, &Params::default());
        assert_eq!(a, b);
    }
}
