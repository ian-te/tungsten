//! A CineStill-style film look for any photo.
//!
//! CineStill 800T is Kodak Vision3 motion-picture stock with the remjet
//! backing removed. Without that anti-halation layer, light bounces off the
//! film base and re-exposes the red-sensitive layer, so every bright light
//! gets a red-orange glow. The stock is balanced for tungsten light, which
//! gives daylight scenes a cool cast with teal shadows.
//!
//! The pipeline here, per pixel, on an 8-bit RGBA buffer in place:
//! 1. Highlights are pooled into a low-resolution mask and blurred twice
//!    (a tight core and a wide falloff); that glow is added to the red
//!    channel in linear light.
//! 2. A tungsten white balance cools the image.
//! 3. An S-curve, faded blacks, split toning (teal shadows, warm highlights),
//!    saturation and a vignette shape the print.
//! 4. Grain: two octaves of value noise, strongest in the midtones.
//!
//! Every size (glow radius, grain cell, vignette) is set relative to the
//! image's short edge, the halation map has the same grid at any size, and
//! each pixel averages the grain over its area, so developing a smaller copy
//! of a photo gives the same result as shrinking the developed full-size one.

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
    /// S-curve contrast. 0–1.
    pub contrast: f32,
    /// Lifted blacks and softened whites. 0–1.
    pub fade: f32,
    /// Colour saturation, 1 = unchanged. 0–2.
    pub saturation: f32,
    /// Corner darkening. 0–1.
    pub vignette: f32,
    /// Grain pattern seed.
    pub seed: u32,
}

impl Params {
    /// 800T: tungsten night stock. Strong halation, cool cast, visible grain.
    pub const NIGHT_800T: Params = Params {
        halation: 0.8,
        halation_spread: 1.0,
        grain: 0.35,
        grain_size: 1.0,
        tungsten: 0.6,
        contrast: 0.45,
        fade: 0.3,
        saturation: 1.05,
        vignette: 0.25,
        seed: 800,
    };

    /// 50D: daylight stock. Fine grain, little cast, a softer glow.
    pub const DAYLIGHT_50D: Params = Params {
        halation: 0.4,
        halation_spread: 0.7,
        grain: 0.12,
        grain_size: 0.8,
        tungsten: 0.1,
        contrast: 0.35,
        fade: 0.15,
        saturation: 1.15,
        vignette: 0.15,
        seed: 50,
    };
}

impl Default for Params {
    fn default() -> Self {
        Params::NIGHT_800T
    }
}

/// Glow colour in linear light: mostly red, some orange.
const HALATION_RGB: [f32; 3] = [1.0, 0.11, 0.025];

/// Applies the look to `rgba` (row-major, 4 bytes per pixel) in place.
/// Alpha is left untouched.
pub fn apply(rgba: &mut [u8], width: usize, height: usize, p: &Params) {
    let (w, h) = (width, height);
    if w == 0 || h == 0 {
        return;
    }
    assert!(rgba.len() >= w * h * 4, "buffer smaller than width × height × 4");

    let to_linear: Vec<f32> = (0..256).map(|v| srgb_to_linear(v as f32 / 255.0)).collect();

    let short = w.min(h) as f32;
    let rgba = &mut rgba[..w * h * 4];
    let halo = halation_map(rgba, w, h, short, p, &to_linear);

    // Tungsten white balance, in linear light.
    let t = p.tungsten.clamp(0.0, 1.0);
    let wb = [1.0 - 0.12 * t, 1.0 - 0.02 * t, 1.0 + 0.1 * t];
    // Print curve, from linear light to display: sRGB encode, S-curve, fade.
    let k = p.contrast.clamp(0.0, 1.0) * 0.55;
    let fade_lo = p.fade.clamp(0.0, 1.0) * 0.075;
    let fade_hi = p.fade.clamp(0.0, 1.0) * 0.035;
    let print: Vec<f32> = (0..=LUT_MAX)
        .map(|i| {
            let v = linear_to_srgb(i as f32 / LUT_MAX as f32);
            let v = v - k * (std::f32::consts::TAU * v).sin() / std::f32::consts::TAU;
            fade_lo + v * (1.0 - fade_lo - fade_hi)
        })
        .collect();
    let sat = p.saturation.clamp(0.0, 3.0);
    let strength = p.halation.max(0.0) * 2.4;

    // Grain cell size in pixels; roughly 1px on a 1500px-short-edge image.
    // Each pixel averages the grain over its area, so smaller images get finer,
    // fainter grain per pixel, as if the full-size result were downscaled.
    let cell = short as f64 / 1500.0 * p.grain_size.clamp(0.25, 4.0) as f64;
    let grain_amp = p.grain.clamp(0.0, 1.0) * 0.16;
    let mut fine = Octave::new(cell, [0.0, 0.0], p.seed, w);
    let mut coarse = Octave::new(cell * 2.0, [17.3, 5.1], p.seed ^ 0x9e37_79b9, w);

    // Vignette from squared distance to the centre, per column and per row.
    let vig = p.vignette.clamp(0.0, 1.0) * 0.5;
    let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
    let inv_d2 = 1.0 / (cx * cx + cy * cy);
    let col_d2: Vec<f32> = (0..w).map(|x| (x as f32 + 0.5 - cx).powi(2) * inv_d2).collect();

    let mut halo_row = vec![0.0f32; halo.w];
    for (y, row) in rgba.chunks_exact_mut(w * 4).take(h).enumerate() {
        let row_d2 = (y as f32 + 0.5 - cy).powi(2) * inv_d2;
        if grain_amp > 0.0 {
            fine.start_row(y);
            coarse.start_row(y);
        }
        halo.fill_row(y, &mut halo_row);
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            let glow = halo.sample_row(&halo_row, x) * strength;

            let mut c = [0.0f32; 3];
            for i in 0..3 {
                let lin = soft_clip((to_linear[px[i] as usize] + HALATION_RGB[i] * glow) * wb[i]);
                c[i] = print[(lin * LUT_MAX as f32) as usize];
            }

            // Split tone: teal shadows, warm highlights; greens lean cyan.
            let lum = luma(c);
            let sh = (1.0 - lum) * (1.0 - lum) * t;
            let hi = lum * lum * t;
            c[0] += -0.045 * sh + 0.035 * hi;
            c[1] += 0.012 * sh + 0.004 * hi;
            c[2] += 0.04 * sh - 0.04 * hi;
            let green_excess = (c[1] - c[0].max(c[2])).max(0.0);
            c[2] += 0.25 * green_excess * t;

            let lum = luma(c);
            let mut m = 1.0;
            if vig > 0.0 {
                let fall = smoothstep(0.12, 1.1, col_d2[x] + row_d2);
                m = 1.0 - vig * fall * fall;
            }
            for v in c.iter_mut() {
                *v = (lum + (*v - lum) * sat) * m;
            }

            if grain_amp > 0.0 {
                let [mono_fine, red, blue] = fine.sample(x);
                let [mono_coarse, _, _] = coarse.sample(x);
                let mono = 0.75 * mono_fine + 0.45 * mono_coarse;
                let lum = luma(c).clamp(0.0, 1.0);
                // Film grain shows most in the midtones and least in deep blacks.
                let amp = grain_amp * (0.35 + 2.6 * lum * (1.0 - lum)) * (0.4 + 0.6 * lum.sqrt());
                c[0] += amp * (mono + 0.35 * red);
                c[1] += amp * mono;
                c[2] += amp * (mono + 0.35 * blue);
            }

            for i in 0..3 {
                px[i] = (c[i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
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

/// The halation glow at reduced resolution, sampled bilinearly one image
/// row at a time.
struct HaloMap {
    data: Vec<f32>,
    w: usize,
    h: usize,
    scale: f32,
    /// Per image column: left cell, right cell, weight of the right one.
    cols: Vec<(usize, usize, f32)>,
}

impl HaloMap {
    fn empty() -> Self {
        HaloMap { data: Vec::new(), w: 0, h: 0, scale: 1.0, cols: Vec::new() }
    }

    fn cell(&self, i: usize) -> (usize, usize, f32) {
        let f = ((i as f32 + 0.5) * self.scale - 0.5).max(0.0);
        let i0 = f as usize;
        (i0, i0 + 1, f - i0 as f32)
    }

    /// Interpolates the map vertically for image row `y` into `out`.
    fn fill_row(&self, y: usize, out: &mut [f32]) {
        if self.data.is_empty() {
            return;
        }
        let (y0, y1, t) = self.cell(y);
        let (y0, y1) = (y0.min(self.h - 1), y1.min(self.h - 1));
        let (a, b) = (&self.data[y0 * self.w..][..self.w], &self.data[y1 * self.w..][..self.w]);
        for ((o, a), b) in out.iter_mut().zip(a).zip(b) {
            *o = a + (b - a) * t;
        }
    }

    fn sample_row(&self, row: &[f32], x: usize) -> f32 {
        match self.cols.get(x) {
            Some(&(x0, x1, t)) => row[x0] + (row[x1] - row[x0]) * t,
            None => 0.0,
        }
    }
}

/// Cells on the halation map's short edge. The grid is fixed relative to the
/// image, so the glow comes out the same at any image size.
const HALO_CELLS: f32 = 256.0;

/// Pools highlight energy into a grid of `HALO_CELLS` on the short edge (each
/// pixel split by area between the cells it overlaps), then blurs it into a
/// tight core and a wide falloff, with radii set relative to the short edge.
fn halation_map(rgba: &[u8], w: usize, h: usize, short: f32, p: &Params, to_linear: &[f32]) -> HaloMap {
    if p.halation <= 0.0 {
        return HaloMap::empty();
    }
    // Cells per pixel; an image smaller than the grid gets one cell per pixel.
    let cells = HALO_CELLS.min(short);
    let scale = cells / short;
    let (mw, mh) = (((w as f32 * scale).ceil() as usize).max(1), ((h as f32 * scale).ceil() as usize).max(1));
    let cols = area_split(w, scale, mw);
    let rows = area_split(h, scale, mh);

    let mut sum = vec![0.0f32; mw * mh];
    let mut line = vec![0.0f32; mw];
    for (row, &(r0, wr)) in rgba.chunks_exact(w * 4).take(h).zip(&rows) {
        line.fill(0.0);
        for (px, &(c0, wc)) in row.chunks_exact(4).zip(&cols) {
            let (r, g, b) = (to_linear[px[0] as usize], to_linear[px[1] as usize], to_linear[px[2] as usize]);
            // Mostly the brightest channel, so coloured lights glow too,
            // but a near-white source glows more than a lit window.
            let v = 0.65 * r.max(g).max(b) + 0.35 * (r + g + b) / 3.0;
            let e = smoothstep(0.6, 1.0, v);
            let e = e * e;
            line[c0] += e * wc;
            line[(c0 + 1).min(mw - 1)] += e * (1.0 - wc);
        }
        let r1 = (r0 + 1).min(mh - 1);
        for (mx, &e) in line.iter().enumerate() {
            sum[r0 * mw + mx] += e * wr;
            sum[r1 * mw + mx] += e * (1.0 - wr);
        }
    }
    // Each cell holds an area-weighted sum; divide by the area it covers.
    let coverage = |split: &[(usize, f32)], n: usize| {
        let mut a = vec![0.0f32; n];
        for &(i, wi) in split {
            a[i] += wi;
            a[(i + 1).min(n - 1)] += 1.0 - wi;
        }
        a
    };
    let (ax, ay) = (coverage(&cols, mw), coverage(&rows, mh));
    for (my, cells_row) in sum.chunks_exact_mut(mw).enumerate() {
        for (c, a) in cells_row.iter_mut().zip(&ax) {
            *c /= (a * ay[my]).max(1e-6);
        }
    }

    let spread = p.halation_spread.clamp(0.1, 4.0);
    let mut core = sum.clone();
    gaussian(&mut core, mw, mh, cells * 0.0075 * spread);
    gaussian(&mut sum, mw, mh, cells * 0.03 * spread);
    for (c, wide) in core.iter_mut().zip(&sum) {
        *c = 0.55 * *c + 1.6 * wide;
    }
    let mut map = HaloMap { data: core, w: mw, h: mh, scale, cols: Vec::new() };
    map.cols = (0..w)
        .map(|x| {
            let (x0, x1, t) = map.cell(x);
            (x0.min(mw - 1), x1.min(mw - 1), t)
        })
        .collect();
    map
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
/// Rows are worked out top to bottom: `start_row`, then `sample` each pixel.
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

    fn new(cell: f64, offset: [f64; 2], seed: u32, width: usize) -> Self {
        let mut o = Octave {
            per_px: 1.0 / cell.max(1e-6),
            offset,
            seed,
            cols: Vec::new(),
            rows: Default::default(),
            line: Vec::new(),
        };
        o.cols = (0..width).map(|x| o.footprint(x, 0)).collect();
        o
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
        // Lattice rows only move down, so each is hashed once and kept while
        // image rows still touch it.
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

    /// The noise over pixel `x` of the current row.
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

/// WebAssembly entry points: the page allocates a buffer in wasm memory,
/// copies the RGBA pixels in, calls `cs_apply`, and reads them back.
#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::{apply, Params};

    #[no_mangle]
    pub extern "C" fn cs_alloc(len: usize) -> *mut u8 {
        let mut v = Vec::<u8>::with_capacity(len);
        let ptr = v.as_mut_ptr();
        std::mem::forget(v);
        ptr
    }

    /// # Safety
    /// `ptr` and `len` must come from one `cs_alloc` call.
    #[no_mangle]
    pub unsafe extern "C" fn cs_free(ptr: *mut u8, len: usize) {
        drop(Vec::from_raw_parts(ptr, 0, len));
    }

    /// # Safety
    /// `ptr` must point to `width * height * 4` bytes from `cs_alloc`.
    #[no_mangle]
    #[allow(clippy::too_many_arguments)]
    pub unsafe extern "C" fn cs_apply(
        ptr: *mut u8,
        width: u32,
        height: u32,
        halation: f32,
        halation_spread: f32,
        grain: f32,
        grain_size: f32,
        tungsten: f32,
        contrast: f32,
        fade: f32,
        saturation: f32,
        vignette: f32,
        seed: u32,
    ) {
        let len = width as usize * height as usize * 4;
        let buf = std::slice::from_raw_parts_mut(ptr, len);
        let p = Params {
            halation,
            halation_spread,
            grain,
            grain_size,
            tungsten,
            contrast,
            fade,
            saturation,
            vignette,
            seed,
        };
        apply(buf, width as usize, height as usize, &p);
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
            .chunks_exact(4)
            .zip(small.chunks_exact(4))
            .flat_map(|(a, b)| (0..3).map(move |c| (a[c] as f32 - b[c] as f32).powi(2)))
            .sum();
        let rmse = (sq / (w * h * 3) as f32).sqrt();
        // The print curve alone, at the lamp's edges, accounts for about 0.5.
        assert!(rmse < 1.0, "developing a smaller copy differs by {rmse:.2} levels RMS");
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
