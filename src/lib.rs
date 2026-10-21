//! Film looks for any photo: CineStill's halation, colour negative stocks and
//! black-and-white, grain, light leaks.
//!
//! The photo is treated as the light that reached the film, and taken through
//! a simplified negative-and-print chain, the way spectral film emulators do
//! (agx-emulsion, Dehancer), reduced to what can run per pixel:
//!
//! 1. Exposure: the photo in linear light, its highlight roll-off undone, so
//!    a lamp is many stops over grey again rather than just "white".
//! 2. Halation: each channel's light, blurred by a tight core and a long tail
//!    (the bounce off the film base), added back with the total light kept.
//!    Mostly red, as only the red layer sits next to the base. A light leak
//!    and the lens's vignette are exposure too.
//! 3. Taking: a 3×3 matrix for how the stock's layers see colour, or one set
//!    of weights for a panchromatic black-and-white film.
//! 4. The negative: density against log exposure, a straight line with a soft
//!    toe, per layer. Grain is added here, in density, per dye layer, strongest
//!    where the stock's datasheet says it is.
//! 5. Interimage: layers push each other apart (DIR couplers), which is where
//!    a stock's saturation comes from.
//! 6. The print: grey is balanced to neutral, then a per-channel S-curve in
//!    log output. Channels with different toes and slopes tint the shadows
//!    and highlights differently, as real film does.
//!
//! Every size (glow, grain, vignette, leak) is set relative to the image's
//! short edge, the halation mask has the same grid at any size, and each pixel
//! takes the grain averaged over its area. So developing a smaller copy of a
//! photo gives the same result as shrinking the developed full-size one, and
//! a [`Developer`] can work on any part of a photo from a mask pooled at any
//! resolution: a preview, a crop at 100 %, or the full photo strip by strip.

pub mod jpeg;
pub mod meta;
#[cfg(target_arch = "wasm32")]
mod wasm;

/// What a stock is, as opposed to how it's set: its colour separation, its
/// negative and print curves, its grain. Taken from datasheets (Kodak E-4050,
/// E-4046, H-1-5219, F-4017; Fuji AF3-0217E) and what photographers report,
/// then set by eye.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Film {
    /// How each layer sees the photo's red, green and blue (rows sum to 1).
    pub taking: [[f32; 3]; 3],
    /// Where each layer's toe starts, in log10 exposure relative to grey.
    pub toe: [f32; 3],
    /// Print contrast per channel, relative to the overall contrast.
    pub print_gamma: [f32; 3],
    /// Interimage effect: how hard the layers push each other apart.
    pub interimage: f32,
    /// A cast the lab leaves in, in stops per channel.
    pub cast: [f32; 3],
    /// Print black and white, in linear light.
    pub black: f32,
    pub white: f32,
    /// Grain amplitude and size relative to Portra 400, and per layer.
    pub grain: f32,
    pub grain_size: f32,
    pub grain_rgb: [f32; 3],
    /// How much of the grain the layers share (1 = as much as their own).
    pub grain_shared: f32,
    /// Black-and-white silver grain grows with density rather than peaking
    /// at the toe as dye clouds do.
    pub silver: bool,
}

/// A matrix that mixes `a` of each channel into the others: below 0 it
/// separates colours further.
const fn crosstalk(a: f32) -> [[f32; 3]; 3] {
    let (d, o) = (1.0 - a + a / 3.0, a / 3.0);
    [[d, o, o], [o, d, o], [o, o, d]]
}

/// `m` with `v` of channel `from` leaking into layer `to`.
const fn leak(m: [[f32; 3]; 3], to: usize, from: usize, v: f32) -> [[f32; 3]; 3] {
    let mut m = m;
    m[to][from] += v;
    m[to][to] -= v;
    m
}

impl Film {
    /// A plain negative and print, with nothing of any stock in particular.
    pub const PLAIN: Film = Film {
        taking: crosstalk(0.0),
        toe: [-1.45; 3],
        print_gamma: [1.0; 3],
        interimage: 0.1,
        cast: [0.0; 3],
        black: 0.002,
        white: 1.6,
        grain: 1.0,
        grain_size: 1.0,
        grain_rgb: [1.0, 0.9, 1.8],
        grain_shared: 0.8,
        silver: false,
    };

    pub const VISION3_500T: Film = Film {
        taking: crosstalk(0.06),
        toe: [-1.6, -1.4, -1.25],
        print_gamma: [1.04, 1.0, 0.97],
        interimage: 0.15,
        black: 0.003,
        grain: 1.4,
        grain_size: 1.2,
        grain_rgb: [1.0, 0.9, 2.2],
        grain_shared: 0.6,
        ..Film::PLAIN
    };

    pub const VISION3_50D: Film = Film {
        taking: crosstalk(0.05),
        toe: [-1.5; 3],
        print_gamma: [1.02, 1.0, 0.99],
        interimage: 0.08,
        grain: 0.5,
        grain_size: 0.7,
        grain_rgb: [1.0, 0.9, 1.6],
        ..Film::PLAIN
    };

    pub const PORTRA_400: Film = Film {
        taking: leak(leak(crosstalk(0.06), 0, 1, 0.04), 1, 2, 0.05),
        toe: [-1.45, -1.45, -1.40],
        print_gamma: [1.03, 1.0, 0.97],
        interimage: 0.10,
        cast: [0.03, 0.0, -0.04],
        ..Film::PLAIN
    };

    pub const EKTAR_100: Film = Film {
        taking: crosstalk(-0.04),
        toe: [-1.45, -1.40, -1.30],
        print_gamma: [1.04, 1.0, 1.0],
        interimage: 0.22,
        black: 0.0015,
        grain: 0.6,
        grain_size: 0.75,
        grain_rgb: [1.0, 0.9, 1.6],
        ..Film::PLAIN
    };

    pub const SUPERIA_400: Film = Film {
        taking: leak(leak(crosstalk(0.05), 1, 2, 0.08), 1, 0, 0.03),
        toe: [-1.35, -1.45, -1.35],
        print_gamma: [1.0, 0.96, 1.0],
        interimage: 0.20,
        cast: [-0.03, 0.0, 0.03],
        grain: 1.2,
        grain_size: 1.1,
        grain_rgb: [1.0, 0.9, 2.0],
        grain_shared: 0.5,
        ..Film::PLAIN
    };

    /// Panchromatic, a little blue-heavy and short of deep red, as its
    /// spectral sensitivity and filter factors say.
    pub const TRI_X_400: Film = Film {
        taking: [[0.24, 0.46, 0.30]; 3],
        toe: [-1.1; 3],
        print_gamma: [1.0; 3],
        interimage: 0.0,
        black: 0.003,
        white: 1.55,
        grain: 1.6,
        grain_size: 1.5,
        grain_rgb: [1.0; 3],
        grain_shared: 0.0,
        silver: true,
        ..Film::PLAIN
    };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// The stock underneath the settings.
    pub film: Film,
    /// Strength of the red glow around highlights. 0–2.
    pub halation: f32,
    /// Glow radius, relative to the image size. 0.25–3.
    pub halation_spread: f32,
    /// Grain amount, 0.5 = the stock as it is. 0–1.
    pub grain: f32,
    /// Grain size, relative to the stock's. 0.5–3.
    pub grain_size: f32,
    /// Daylight on tungsten film: the cyan cast it leaves. 0–1.
    pub tungsten: f32,
    /// Print balance, cool to warm. -1–1.
    pub warmth: f32,
    /// Print balance, green to magenta. -1–1.
    pub tint: f32,
    /// Overall contrast of negative and print. 0–1.
    pub contrast: f32,
    /// Lifted blacks and softened whites, as from a flashed print. 0–1.
    pub fade: f32,
    /// Colour, 1 = the stock as it is, 0 = none. 0–2.
    pub saturation: f32,
    /// Black and white, through Tri-X's panchromatic response. 0–1.
    pub mono: f32,
    /// Corner darkening. 0–1.
    pub vignette: f32,
    /// A warm light leak from the film's edge. 0–1.
    pub leak: f32,
    /// Grain pattern and light-leak placement.
    pub seed: u32,
}

impl Params {
    /// Neutral settings on a plain film.
    pub const NONE: Params = Params {
        film: Film::PLAIN,
        halation: 0.0,
        halation_spread: 1.0,
        grain: 0.0,
        grain_size: 1.0,
        tungsten: 0.0,
        warmth: 0.0,
        tint: 0.0,
        contrast: 0.5,
        fade: 0.0,
        saturation: 1.0,
        mono: 0.0,
        vignette: 0.0,
        leak: 0.0,
        seed: 0,
    };

    /// CineStill 800T: Vision3 500T without remjet, shot in daylight. Strong
    /// halation, a cyan cast, visible grain.
    pub const NIGHT_800T: Params = Params {
        film: Film::VISION3_500T,
        halation: 0.8,
        grain: 0.5,
        tungsten: 0.6,
        contrast: 0.75,
        fade: 0.05,
        vignette: 0.25,
        seed: 800,
        ..Params::NONE
    };

    /// CineStill 50D: Vision3 50D without remjet. Fine grain, a softer glow.
    pub const DAYLIGHT_50D: Params = Params {
        film: Film::VISION3_50D,
        halation: 0.5,
        halation_spread: 0.8,
        grain: 0.5,
        contrast: 0.5,
        fade: 0.1,
        vignette: 0.15,
        seed: 50,
        ..Params::NONE
    };

    /// Kodak Portra 400: warm, soft, kind to skin, wide latitude.
    pub const PORTRA_400: Params = Params {
        film: Film::PORTRA_400,
        grain: 0.5,
        contrast: 0.5,
        fade: 0.1,
        vignette: 0.1,
        seed: 400,
        ..Params::NONE
    };

    /// Kodak Ektar 100: saturated reds and blues, contrast, very fine grain.
    pub const EKTAR_100: Params = Params {
        film: Film::EKTAR_100,
        grain: 0.5,
        contrast: 0.75,
        vignette: 0.2,
        seed: 100,
        ..Params::NONE
    };

    /// Fujicolor Superia 400: green-leaning, cool, punchy grain.
    pub const SUPERIA_400: Params = Params {
        film: Film::SUPERIA_400,
        grain: 0.5,
        contrast: 0.7,
        vignette: 0.15,
        seed: 401,
        ..Params::NONE
    };

    /// Kodak Tri-X 400: black and white, punchy contrast, gritty grain.
    pub const TRI_X_400: Params = Params {
        film: Film::TRI_X_400,
        grain: 0.5,
        contrast: 0.83,
        fade: 0.05,
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

/// Halation per channel at full strength: mostly red, a little green (an
/// orange core where the red saturates); blue's 0.012 is left out.
const HALATION_RGB: [f32; 2] = [0.25, 0.02];
/// Extra stops given to clipped highlights as a source of halation.
const HALATION_BOOST: f32 = 3.0;
/// Light-leak colour, as exposure: orange-red.
const LEAK_RGB: [f32; 3] = [1.0, 0.36, 0.08];
/// Tri-X's weights for red, green and blue.
const PANCHROMATIC: [f32; 3] = [0.24, 0.46, 0.30];
/// Density per log10 exposure of the negative's straight line.
const NEG_GAMMA: f32 = 0.6;
/// Width of the negative's toe, in log10 exposure.
const TOE_WIDTH: f32 = 0.2;
/// Stops per log10 exposure.
const STOPS: f32 = std::f32::consts::LOG2_10;
/// The brightest exposure the tables cover, relative to grey: 9 stops over.
const EXPOSURE_MAX: f32 = 0.18 * 512.0;
/// The range of print densities (relative to grey) the print table covers.
const Z_RANGE: f32 = 2.5;

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
const HALO_CELLS: f32 = 512.0;

/// A photo's light (red and green, the channels that halate), pooled into a
/// grid of `HALO_CELLS` on the short edge. It doesn't depend on any setting,
/// so it is worked out once per photo, at full size, and serves every
/// rendering of it.
#[derive(Clone, Debug, Default)]
pub struct Mask {
    data: [Vec<f32>; 2],
    w: usize,
    h: usize,
    /// Cells along the photo's short edge.
    cells: f32,
    /// The last glow worked out from it, and the spread it was for: a preview
    /// is developed again on every change of setting, the glow only changes
    /// with the spread.
    glow: std::cell::RefCell<Option<(f32, Glow)>>,
}

/// The blurred red and green light, shared by the developers that use it.
type Glow = std::rc::Rc<[Vec<f32>; 2]>;

/// Pools a photo's light into a [`Mask`], a strip of rows at a time, each
/// pixel split by area between the cells it overlaps.
pub struct MaskBuilder {
    w: usize,
    cells: f32,
    mw: usize,
    mh: usize,
    cols: Vec<(usize, f32)>,
    rows: Vec<(usize, f32)>,
    sum: [Vec<f32>; 2],
    line: [Vec<f32>; 2],
    y: usize,
    linear: [f32; 256],
    boost: [f32; 256],
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
            sum: [vec![0.0; mw * mh], vec![0.0; mw * mh]],
            line: [vec![0.0; mw], vec![0.0; mw]],
            y: 0,
            linear: linear_table(),
            boost: halation_boost(),
        }
    }

    /// Adds the next whole rows of the photo, `w * 4` bytes each, top to bottom.
    pub fn add_rows(&mut self, rgba: &[u8]) {
        let (mw, mh) = (self.mw, self.mh);
        for row in rgba.chunks_exact(self.w * 4) {
            let Some(&(r0, wr)) = self.rows.get(self.y) else { return };
            self.y += 1;
            let r1 = (r0 + 1).min(mh - 1);
            for line in &mut self.line {
                line.fill(0.0);
            }
            for (px, &(c0, wc)) in row.as_chunks::<4>().0.iter().zip(&self.cols) {
                let e = exposure(px, &self.linear);
                let boost = self.boost[px[0].max(px[1]).max(px[2]) as usize];
                for (line, e) in self.line.iter_mut().zip(e) {
                    line[c0] += e * boost * wc;
                    line[(c0 + 1).min(mw - 1)] += e * boost * (1.0 - wc);
                }
            }
            for c in 0..2 {
                let line = &self.line[c];
                let sum = &mut self.sum[c];
                for (mx, &e) in line.iter().enumerate() {
                    sum[r0 * mw + mx] += e * wr;
                    sum[r1 * mw + mx] += e * (1.0 - wr);
                }
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
        for sum in &mut self.sum {
            for (my, cells_row) in sum.chunks_exact_mut(self.mw).enumerate() {
                for (c, a) in cells_row.iter_mut().zip(&ax) {
                    *c /= (a * ay[my]).max(1e-6);
                }
            }
        }
        Mask { data: self.sum, w: self.mw, h: self.mh, cells: self.cells, glow: Default::default() }
    }
}

/// Develops a photo of `w`×`h` pixels, or any part of it, with one set of
/// [`Params`]. The photo can be any size: the mask may come from a larger
/// copy of it, and parts can be developed in any order.
pub struct Developer {
    w: usize,
    h: usize,
    halo: HaloMap,
    /// Halation added to red and green (blue's is too faint to matter).
    halo_amount: [f32; 2],
    linear: [f32; 256],
    taking: [[f32; 3]; 3],
    /// Per layer, against the fourth root of exposure: density, and how
    /// grainy the negative is there.
    density: [Vec<f32>; 3],
    granularity: [Vec<f32>; 3],
    /// Per channel, against print density relative to grey: output, 0–255,
    /// and half its second derivative, which is how far grain of unit
    /// variance shifts the mean there.
    print: [Vec<f32>; 3],
    print_bend: [Vec<f32>; 3],
    /// The grain's variance per pixel at this size, before granularity.
    grain_var: [f32; 3],
    grey: [f32; 3],
    separation: f32,
    grain_amp: [f32; 3],
    grain_shared: f32,
    vig: f32,
    fine: Octave,
    coarse: Octave,
    leak: Leak,
    /// The columns the last [`Developer::develop`] call covered, and what
    /// depends on them only.
    region: Option<(usize, usize)>,
    col_d2: Vec<f32>,
    halo_cols: Vec<(usize, usize, f32)>,
    halo_row: [Vec<f32>; 2],
}

const TABLE: usize = 4096;

impl Developer {
    pub fn new(mask: &Mask, w: usize, h: usize, p: &Params) -> Self {
        let short = w.min(h).max(1) as f32;
        let f = &p.film;
        let mono = p.mono.clamp(0.0, 1.0);

        // Taking: the stock's layers, or the panchromatic weights.
        let mut taking = f.taking;
        for row in &mut taking {
            for (v, pan) in row.iter_mut().zip(PANCHROMATIC) {
                *v += (pan - *v) * mono;
            }
        }

        // The negative, per layer, tabulated against q = (e / EXPOSURE_MAX)^¼,
        // which spends the table's entries evenly across the stops.
        let toe: [f32; 3] = std::array::from_fn(|c| f.toe[c] + (f.toe.iter().sum::<f32>() / 3.0 - f.toe[c]) * mono);
        let neg = |c: usize, x: f32| NEG_GAMMA * TOE_WIDTH * softplus((x - toe[c]) / TOE_WIDTH);
        let exposure_of = |i: usize| EXPOSURE_MAX * (i as f32 / (TABLE - 1) as f32).powi(4);
        let log_rel = |e: f32| (e.max(1e-7) / 0.18).log10();
        let density: [Vec<f32>; 3] = std::array::from_fn(|c| (0..TABLE).map(|i| neg(c, log_rel(exposure_of(i)))).collect());
        let grey_density: [f32; 3] = std::array::from_fn(|c| neg(c, 0.0));
        // Granularity: dye clouds peak just above the toe (Vision3's curves), silver
        // grows with density (Selwyn).
        let granularity: [Vec<f32>; 3] = std::array::from_fn(|c| {
            (0..TABLE)
                .map(|i| {
                    let x = log_rel(exposure_of(i));
                    if f.silver {
                        (neg(c, x) / grey_density[c]).sqrt().clamp(0.5, 1.3)
                    } else {
                        dye_granularity(x * STOPS)
                    }
                })
                .collect()
        });

        // Interimage, then grey balance: a mid-grey exposure prints mid-grey,
        // apart from the casts.
        let sat = p.saturation.clamp(0.0, 3.0) * (1.0 + f.interimage * (1.0 - mono));
        let grey_e: [f32; 3] = std::array::from_fn(|r| (0..3).map(|c| taking[r][c] * 0.18).sum());
        let grey_d: [f32; 3] = std::array::from_fn(|c| neg(c, log_rel(grey_e[c])));
        let grey_mean = grey_d.iter().sum::<f32>() / 3.0;
        let tung = p.tungsten.clamp(0.0, 1.0);
        let (warm, tint) = (p.warmth.clamp(-1.0, 1.0), p.tint.clamp(-1.0, 1.0));
        let cast: [f32; 3] = std::array::from_fn(|c| {
            let stops = f.cast[c] + [-0.58, 0.08, 0.5][c] * tung + [0.3, 0.0, -0.3][c] * warm + [0.0, -0.25, 0.0][c] * tint;
            stops * (1.0 - mono) / STOPS * NEG_GAMMA
        });
        let grey: [f32; 3] = std::array::from_fn(|c| grey_mean + (grey_d[c] - grey_mean) * sat - cast[c]);

        // The print: an S-curve in log output through black, grey and white,
        // with the slope at grey set by contrast, per channel.
        let fade = p.fade.clamp(0.0, 1.0);
        let black = f.black * (1.0 + 12.0 * fade);
        let white = f.white * (1.0 - 0.25 * fade);
        let (yb, yw) = (black.log10(), white.log10());
        let mid = (0.18f32.log10() - yb) / (yw - yb);
        let system = 0.8 + 0.6 * p.contrast.clamp(0.0, 1.0);
        let print: [Vec<f32>; 3] = std::array::from_fn(|c| {
            let pg = f.print_gamma[c] + (1.0 - f.print_gamma[c]) * mono;
            let slope = system * pg / (NEG_GAMMA * (yw - yb) * mid * (1.0 - mid));
            let logit_mid = (mid / (1.0 - mid)).ln();
            (0..TABLE)
                .map(|i| {
                    let z = (i as f32 / (TABLE - 1) as f32 * 2.0 - 1.0) * Z_RANGE;
                    let y = yb + (yw - yb) * sigmoid(slope * z + logit_mid);
                    linear_to_srgb(10f32.powf(y).min(1.0)) * 255.0
                })
                .collect()
        });

        // Grain cells in pixels: Portra 400's fine dye clouds are about 1/2400 of
        // a 35mm frame's short edge; the coarse octave is 2.3 times larger.
        let cell = short as f64 / 2400.0 * (p.grain_size.clamp(0.25, 4.0) * f.grain_size) as f64;
        let amp = p.grain.clamp(0.0, 1.0) * 2.0 * f.grain * GRAIN_DENSITY;
        let shared = f.grain_shared * (1.0 - mono);
        let norm = 1.0 / (1.0 + 2.0 * shared / 3.0 + shared * shared / 3.0).sqrt();
        let grain_amp: [f32; 3] = std::array::from_fn(|c| amp * (f.grain_rgb[c] + (1.0 - f.grain_rgb[c]) * mono) * norm);

        let dz = 2.0 * Z_RANGE / (TABLE - 1) as f32;
        let print_bend: [Vec<f32>; 3] = std::array::from_fn(|c| {
            let t = &print[c];
            (0..TABLE).map(|i| if i == 0 || i == TABLE - 1 { 0.0 } else { 0.5 * (t[i + 1] - 2.0 * t[i] + t[i - 1]) / (dz * dz) }).collect()
        });
        let fine = Octave::new(cell, [0.0, 0.0], p.seed);
        let coarse = Octave::new(cell * 2.3, [17.3, 5.1], p.seed ^ 0x9e37_79b9);
        // An octave's variance is (1/3)·mean_sq² per pixel: (26/35)²/3 for a point.
        let var_n = GRAIN_OCTAVES * GRAIN_OCTAVES * (2.3 * 2.3 * fine.mean_sq * fine.mean_sq + coarse.mean_sq * coarse.mean_sq) as f32 / 3.0;
        let grain_var = grain_amp.map(|a| a * a * var_n);

        let strength = p.halation.max(0.0) * 2.4;
        Developer {
            w,
            h,
            halo: HaloMap::new(mask, short, p),
            halo_amount: [HALATION_RGB[0] * strength, HALATION_RGB[1] * strength],
            linear: linear_table(),
            taking,
            density,
            granularity,
            print,
            print_bend,
            grain_var,
            grey,
            separation: sat,
            grain_amp,
            grain_shared: if mono > 0.5 { -1.0 } else { shared },
            vig: p.vignette.clamp(0.0, 1.0) * 0.6,
            fine,
            coarse,
            leak: Leak::new(p.leak.clamp(0.0, 1.0), p.seed, w, h),
            region: None,
            col_d2: Vec::new(),
            halo_cols: Vec::new(),
            halo_row: [Vec::new(), Vec::new()],
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
        let has_glow = self.halo_amount[0] > 0.0 && !self.halo.data[0].is_empty();
        let has_grain = self.grain_amp.iter().any(|&a| a > 0.0);
        let keep = self.halo_amount.map(|a| 1.0 / (1.0 + a));
        let q_scale = 1.0 / EXPOSURE_MAX;
        let t_max = (TABLE - 1) as f32;
        let z_to_index = t_max / (2.0 * Z_RANGE);

        for (dy, row) in rgba.chunks_exact_mut(rw * 4).take(rh).enumerate() {
            let y = y0 + dy;
            let row_d2 = (y as f32 + 0.5 - cy).powi(2) * inv_d2;
            if has_grain {
                self.fine.start_row(y);
                self.coarse.start_row(y);
            }
            if has_glow {
                for c in 0..2 {
                    self.halo.fill_row(c, y, &mut self.halo_row[c]);
                }
            }
            let leak_row = self.leak.row(y);
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                // 1–2. Exposure, halation (total light kept), leak, vignette.
                let mut e = exposure(px, &self.linear);
                if has_glow {
                    let (x0, x1, t) = self.halo_cols[x];
                    for c in 0..2 {
                        let row = &self.halo_row[c];
                        let glow = row[x0] + (row[x1] - row[x0]) * t;
                        e[c] = (e[c] + self.halo_amount[c] * glow) * keep[c];
                    }
                }
                let leak = self.leak.at(x, leak_row);
                let fall = if self.vig > 0.0 { smoothstep(0.12, 1.1, self.col_d2[x] + row_d2) } else { 0.0 };
                let m = 1.0 - self.vig * fall * fall;
                for c in 0..3 {
                    e[c] = (e[c] + LEAK_RGB[c] * leak) * m;
                }

                // 3–4. Taking, the negative, grain in density.
                let mut d = [0.0f32; 3];
                let mut gran = [0.0f32; 3];
                for r in 0..3 {
                    let t = &self.taking[r];
                    let er = t[0] * e[0] + t[1] * e[1] + t[2] * e[2];
                    let q = ((er * q_scale).clamp(0.0, 1.0)).sqrt().sqrt() * t_max;
                    let i = q as usize;
                    d[r] = self.density[r][i];
                    gran[r] = self.granularity[r][i];
                }
                if has_grain {
                    let fine = self.fine.sample(x);
                    let coarse = self.coarse.sample(x);
                    let n: [f32; 3] = std::array::from_fn(|c| (2.3 * fine[c] + coarse[c]) * GRAIN_OCTAVES);
                    let mean = (n[0] + n[1] + n[2]) / 3.0;
                    for c in 0..3 {
                        // Black and white: one silver layer, so one pattern.
                        let nc = if self.grain_shared < 0.0 { mean * 3f32.sqrt() } else { n[c] + self.grain_shared * mean };
                        d[c] += self.grain_amp[c] * gran[c] * nc;
                    }
                }

                // 5–6. Interimage, grey balance, the print.
                let mean = (d[0] + d[1] + d[2]) / 3.0;
                for c in 0..3 {
                    let z = mean + (d[c] - mean) * self.separation - self.grey[c];
                    let i = ((z + Z_RANGE) * z_to_index).clamp(0.0, t_max) as usize;
                    let mut out = self.print[c][i];
                    if has_grain {
                        // The curve's bend would shift the mean by the grain's variance:
                        // take that back, so grain is unbiased at any size. (Read where
                        // the grain put it; the bend changes too slowly for that to matter.)
                        let g = gran[c];
                        out -= self.print_bend[c][i] * self.grain_var[c] * g * g;
                    }
                    px[c] = (out.clamp(0.0, 255.0) + 0.5) as u8;
                }
            }
        }
    }
}

/// Grain density amplitude at grain 0.5 on Portra 400, per unit of noise.
const GRAIN_DENSITY: f32 = 0.025;
/// Brings 2.3 × fine + coarse octave noise to unit variance at full resolution
/// (each octave's point variance is (26/35)² / 3).
const GRAIN_OCTAVES: f32 = 0.929_7;

/// How grainy a colour negative is at `stops` from grey, relative to its
/// straight line: low under the toe, a peak just above it, gently falling in
/// the highlights (Kodak Vision3 granularity curves).
fn dye_granularity(stops: f32) -> f32 {
    const PTS: [(f32, f32); 5] = [(-5.0, 0.4), (-2.3, 1.5), (-1.0, 1.0), (4.0, 1.0), (6.0, 0.8)];
    if stops <= PTS[0].0 {
        return PTS[0].1;
    }
    for w in PTS.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if stops <= x1 {
            return y0 + (y1 - y0) * (stops - x0) / (x1 - x0);
        }
    }
    PTS[4].1
}

fn softplus(x: f32) -> f32 {
    if x > 20.0 {
        x
    } else {
        x.exp().ln_1p()
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// The photo's 8-bit values in linear light.
fn linear_table() -> [f32; 256] {
    std::array::from_fn(|v| srgb_to_linear(v as f32 / 255.0))
}

/// A pixel's exposure: one gain for all three channels, which keeps the
/// colour of bright lights, set by the pixel's luminance, so a bright but
/// coloured pixel (skin in sunlight) isn't lifted as if it were white.
fn exposure(px: &[u8; 4], linear: &[f32; 256]) -> [f32; 3] {
    let e = [linear[px[0] as usize], linear[px[1] as usize], linear[px[2] as usize]];
    let y = 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
    let g = if y > 0.5 { expand(y) / y } else { 1.0 };
    e.map(|v| v * g)
}

/// Undoes a camera's shoulder above `KNEE`: white (1.0) becomes `TOP`.
fn expand(x: f32) -> f32 {
    const KNEE: f32 = 0.5;
    const TOP: f32 = 3.0;
    if x <= KNEE {
        return x;
    }
    let c = 1.0 - (1.0 - KNEE) / (TOP - KNEE);
    let u = (x - KNEE) / (1.0 - KNEE);
    KNEE + (x - KNEE) / (1.0 - c * u)
}

/// Per 8-bit value of a pixel's brightest channel: extra light for clipped
/// highlights as a source of halation, as a lamp that clipped was far
/// brighter still. Bright skin or cloud that didn't clip gets none.
fn halation_boost() -> [f32; 256] {
    std::array::from_fn(|v| 2f32.powf(HALATION_BOOST * smoothstep(0.95, 1.0, v as f32 / 255.0).powi(2)))
}

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

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The blurred halation glow of red and green on the mask's grid, sampled
/// bilinearly for the photo being developed, one row at a time.
struct HaloMap {
    data: Glow,
    w: usize,
    h: usize,
    /// Mask cells per pixel of the photo being developed.
    scale: f32,
}

impl HaloMap {
    /// Blurs each channel of `mask` with a tight core, the ring of the bounce
    /// off the base, and a long tail, as fractions of the short edge.
    fn new(mask: &Mask, short: f32, p: &Params) -> Self {
        if p.halation <= 0.0 || mask.data[0].is_empty() {
            return HaloMap { data: Default::default(), w: 0, h: 0, scale: 1.0 };
        }
        const KERNEL: [(f32, f32); 3] = [(0.003, 0.25), (0.012, 0.35), (0.04, 0.40)];
        let (mw, mh, cells) = (mask.w, mask.h, mask.cells);
        let spread = p.halation_spread.clamp(0.1, 4.0);
        let mut glow = mask.glow.borrow_mut();
        let data = match &*glow {
            Some((s, data)) if *s == spread => data.clone(),
            _ => {
                let data = std::rc::Rc::new(mask.data.clone().map(|src| {
                    let mut out = vec![0.0f32; src.len()];
                    for (sigma, weight) in KERNEL {
                        let mut b = src.clone();
                        gaussian(&mut b, mw, mh, cells * sigma * spread);
                        for (o, v) in out.iter_mut().zip(&b) {
                            *o += weight * v;
                        }
                    }
                    out
                }));
                *glow = Some((spread, data.clone()));
                data
            }
        };
        HaloMap { data, w: mw, h: mh, scale: cells / short }
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

    /// Interpolates channel `c` vertically for image row `y` into `out`.
    fn fill_row(&self, c: usize, y: usize, out: &mut Vec<f32>) {
        let (y0, y1, t) = self.cell(y);
        let (y0, y1) = (y0.min(self.h - 1), y1.min(self.h - 1));
        let data = &self.data[c];
        let (a, b) = (&data[y0 * self.w..][..self.w], &data[y1 * self.w..][..self.w]);
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
    /// The mean over positions of a footprint's sum of squared weights.
    mean_sq: f64,
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
    const MAX_PER_PX: f64 = 1.5;

    fn new(cell: f64, offset: [f64; 2], seed: u32) -> Self {
        let per_px = 1.0 / cell.max(1e-6);
        let phases = 64;
        let mean_sq = if per_px > Self::MAX_PER_PX {
            26.0 / 35.0 / per_px
        } else {
            (0..phases)
            .map(|k| {
                let a = k as f64 / phases as f64;
                let (_, w, n) = Octave::weights(a, a + per_px);
                w[..n].iter().map(|w| w * w).sum::<f64>()
            })
            .sum::<f64>()
            / phases as f64
        };
        Octave { per_px, mean_sq, offset, seed, cols: Vec::new(), rows: Default::default(), line: Vec::new() }
    }

    /// The lattice points over [a, b] (in cells), from the first, and the
    /// smoothstep interpolation kernel integrated across it, divided by its width.
    fn weights(a: f64, b: f64) -> (f64, [f64; TAPS], usize) {
        let start = a.floor();
        let n = ((b.ceil() - start) as usize + 1).min(TAPS);
        let mut weights = [0.0; TAPS];
        for (k, wk) in weights.iter_mut().enumerate().take(n) {
            let l = start + k as f64;
            *wk = (kernel_integral(b - l) - kernel_integral(a - l)) / (b - a);
        }
        (start, weights, n)
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
        let (start, w, n) = Octave::weights(a, b);
        // Interpolated noise is fainter between lattice points than on them;
        // even that out, keeping the fall in variance that averaging over a
        // larger pixel brings, so no lattice shows through.
        let even = (self.mean_sq / w[..n].iter().map(|w| w * w).sum::<f64>()).sqrt();
        let mut weights = [0.0; TAPS];
        for (o, w) in weights.iter_mut().zip(&w[..n]) {
            *o = (w * even) as f32;
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
        let i = (100 * w + 107) * 4;
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

    /// A dark scene with a grey wall and a lamp, at `k` times the size.
    fn scene(w: usize, h: usize, k: usize) -> Vec<u8> {
        let mut img = solid(w * k, h * k, 12);
        for y in 0..h * k {
            for x in 0..w * k {
                let lamp = (40 * k..52 * k).contains(&x) && (30 * k..42 * k).contains(&y);
                let wall = x > 70 * k && y > 20 * k;
                if lamp || wall {
                    let i = (y * w * k + x) * 4;
                    img[i..i + 3].fill(if lamp { 255 } else { 120 });
                }
            }
        }
        img
    }

    /// Develops the scene at 4× and shrinks it, and develops a 1× copy.
    fn both_ways(w: usize, h: usize, p: &Params) -> (Vec<u8>, Vec<u8>) {
        let k = 4;
        let mut big = scene(w, h, k);
        let mut small = shrink(&big, w * k, h * k, k);
        apply(&mut big, w * k, h * k, p);
        apply(&mut small, w, h, p);
        (shrink(&big, w * k, h * k, k), small)
    }

    #[test]
    fn a_smaller_copy_develops_the_same() {
        let (w, h) = (180, 120);
        let (big, small) = both_ways(w, h, &Params { grain: 0.0, ..Params::default() });
        let sq: f32 = big
            .as_chunks::<4>()
            .0
            .iter()
            .zip(small.as_chunks::<4>().0)
            .flat_map(|(a, b)| (0..3).map(move |c| (a[c] as f32 - b[c] as f32).powi(2)))
            .sum();
        let rmse = (sq / (w * h * 3) as f32).sqrt();
        // The film curves alone, at the lamp's and the wall's edges, account for about 1.
        assert!(rmse < 1.5, "developing a smaller copy differs by {rmse:.2} levels RMS");
    }

    #[test]
    fn a_smaller_copy_has_the_same_grain() {
        // The same grain statistically, on the wall: same brightness, same strength.
        let (w, h) = (180, 120);
        let (big, small) = both_ways(w, h, &Params { halation: 0.0, vignette: 0.0, ..Params::default() });
        let stats = |img: &[u8]| {
            let wall: Vec<f64> = (40..110).flat_map(|y| (90..170).map(move |x| (y * w + x) * 4 + 1)).map(|i| img[i] as f64).collect();
            let mean = wall.iter().sum::<f64>() / wall.len() as f64;
            (mean, (wall.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / wall.len() as f64).sqrt())
        };
        let ((mb, sb), (ms, ss)) = (stats(&big), stats(&small));
        assert!((mb - ms).abs() < 0.5, "brightness {mb:.2} vs {ms:.2}");
        assert!((sb / ss - 1.0).abs() < 0.15, "grain {sb:.2} vs {ss:.2}");
    }

    #[test]
    fn a_blue_sky_stays_blue() {
        // Halation comes from each channel's own light: a bright blue sky over dark
        // trees gets no pink wash, only a faint fringe at the edge.
        let (w, h) = (300, 200);
        let mut img = solid(w, h, 20);
        for px in img.chunks_mut(4).take(w * 120) {
            px[..3].copy_from_slice(&[110, 160, 230]);
        }
        let p = Params { grain: 0.0, vignette: 0.0, ..Params::default() };
        let (mut with, mut without) = (img.clone(), img);
        apply(&mut with, w, h, &p);
        apply(&mut without, w, h, &Params { halation: 0.0, ..p });
        let i = (40 * w + 150) * 4;
        assert!(with[i].abs_diff(without[i]) <= 2, "sky red {} vs {}", with[i], without[i]);
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
