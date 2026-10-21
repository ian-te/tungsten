//! Saving a developed photo as JPEG without ever holding it whole: the
//! encoder asks for rows top to bottom, and they are read from a
//! [`RowSource`] and developed a strip at a time. So the only limit on a
//! photo's size is JPEG's own, 65,535 pixels a side.

use std::cell::RefCell;

use jpeg_encoder::{rgb_to_ycbcr, Encoder, ImageBuffer, JpegColorType};

use crate::meta::{clean_exif, is_display_p3, SourceMeta};
use crate::Developer;

/// Rows developed at a time.
const STRIP: usize = 128;

/// Where the photo's original pixels come from.
pub trait RowSource {
    /// Fills `out` with the RGBA pixels of rows `y0..` of the photo, as many
    /// whole rows as fit.
    fn rows(&mut self, y0: usize, out: &mut [u8]);
}

/// Pixels already in memory, row-major RGBA.
pub struct Pixels<'a> {
    pub rgba: &'a [u8],
    pub width: usize,
}

impl RowSource for Pixels<'_> {
    fn rows(&mut self, y0: usize, out: &mut [u8]) {
        let start = y0 * self.width * 4;
        let n = out.len().min(self.rgba.len().saturating_sub(start));
        out[..n].copy_from_slice(&self.rgba[start..start + n]);
    }
}

/// A printed film border around the photo: bands above and below (RGBA,
/// the full width of the result, edge markings and all) and plain sides.
#[derive(Clone, Debug, Default)]
pub struct Border {
    pub side: usize,
    pub top: Vec<u8>,
    pub top_rows: usize,
    pub bottom: Vec<u8>,
    pub bottom_rows: usize,
    pub fill: [u8; 3],
}

/// How to save, beyond the pixels.
#[derive(Clone, Debug)]
pub struct Options<'a> {
    /// 1–100.
    pub quality: u8,
    pub meta: &'a SourceMeta,
    /// Keep where the photo was taken, if the original says.
    pub keep_location: bool,
    /// The pixels are in Display P3 (read that way from a P3 original), so
    /// its profile goes with them.
    pub display_p3: bool,
    pub border: Option<&'a Border>,
}

/// Develops a `width`×`height` photo read from `source` with `dev` and
/// encodes the result as JPEG.
pub fn encode<S: RowSource>(
    source: S,
    dev: Developer,
    width: usize,
    height: usize,
    opts: &Options,
) -> Result<Vec<u8>, String> {
    let empty = Border::default();
    let border = opts.border.unwrap_or(&empty);
    let out_w = width + 2 * border.side;
    let out_h = height + border.top_rows + border.bottom_rows;
    if width == 0 || height == 0 {
        return Err("the photo is empty".into());
    }
    if out_w > u16::MAX as usize || out_h > u16::MAX as usize {
        return Err(format!("{out_w}×{out_h} is larger than JPEG allows (65,535 pixels a side)"));
    }
    if border.top.len() < out_w * border.top_rows * 4 || border.bottom.len() < out_w * border.bottom_rows * 4 {
        return Err("the border bands are smaller than the photo".into());
    }

    let mut bytes = Vec::new();
    let mut enc = Encoder::new(&mut bytes, opts.quality.clamp(1, 100));
    if let Some(exif) = opts.meta.exif.as_deref().and_then(|e| clean_exif(e, opts.keep_location, out_w as u32, out_h as u32)) {
        // Too large for one segment is rare; such EXIF is left out.
        let _ = enc.add_app_segment(1, exif);
    }
    if opts.display_p3 {
        if let Some(icc) = opts.meta.icc.as_deref().filter(|icc| is_display_p3(icc)) {
            enc.add_icc_profile(icc).map_err(|e| e.to_string())?;
        }
    }
    let image = Rows {
        w: width as u16,
        out_w: out_w as u16,
        out_h: out_h as u16,
        h: height,
        border,
        state: RefCell::new(State { source, dev, strip: Vec::new(), y0: 0, rows: 0 }),
    };
    enc.encode_image(image).map_err(|e| e.to_string())?;
    Ok(bytes)
}

struct State<S> {
    source: S,
    dev: Developer,
    /// Developed rows `y0..y0 + rows` of the photo.
    strip: Vec<u8>,
    y0: usize,
    rows: usize,
}

struct Rows<'a, S> {
    w: u16,
    h: usize,
    out_w: u16,
    out_h: u16,
    border: &'a Border,
    state: RefCell<State<S>>,
}

impl<S: RowSource> ImageBuffer for Rows<'_, S> {
    fn get_jpeg_color_type(&self) -> JpegColorType {
        JpegColorType::Ycbcr
    }

    fn width(&self) -> u16 {
        self.out_w
    }

    fn height(&self) -> u16 {
        self.out_h
    }

    fn fill_buffers(&self, y: u16, buffers: &mut [Vec<u8>; 4]) {
        let mut push = |r: u8, g: u8, b: u8| {
            let (y, cb, cr) = rgb_to_ycbcr(r, g, b);
            buffers[0].push(y);
            buffers[1].push(cb);
            buffers[2].push(cr);
        };
        let b = self.border;
        let (y, w, out_w) = (y as usize, self.w as usize, self.out_w as usize);
        let band = if y < b.top_rows {
            Some(&b.top[y * out_w * 4..][..out_w * 4])
        } else if y >= b.top_rows + self.h {
            Some(&b.bottom[(y - b.top_rows - self.h) * out_w * 4..][..out_w * 4])
        } else {
            None
        };
        if let Some(band) = band {
            for px in band.as_chunks::<4>().0 {
                push(px[0], px[1], px[2]);
            }
            return;
        }

        let py = y - b.top_rows;
        let mut s = self.state.borrow_mut();
        if py < s.y0 || py >= s.y0 + s.rows {
            let rows = STRIP.min(self.h - py);
            let State { source, dev, strip, .. } = &mut *s;
            strip.resize(w * rows * 4, 0);
            source.rows(py, strip);
            dev.develop(strip, 0, py, w, rows);
            (s.y0, s.rows) = (py, rows);
        }
        let row = &s.strip[(py - s.y0) * w * 4..][..w * 4];
        let [fr, fg, fb] = b.fill;
        for _ in 0..b.side {
            push(fr, fg, fb);
        }
        for px in row.as_chunks::<4>().0 {
            push(px[0], px[1], px[2]);
        }
        for _ in 0..b.side {
            push(fr, fg, fb);
        }
    }
}

#[cfg(all(test, feature = "cli"))]
mod tests {
    use super::*;
    use crate::{Mask, Params};

    #[test]
    fn saves_with_a_border_and_the_original_details() {
        let (w, h) = (40, 30);
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| if i % 4 == 3 { 255 } else { 128 }).collect();
        let out_w = w + 2 * 3;
        let band = |rows: usize| [20u8, 20, 20, 255].repeat(out_w * rows);
        let border = Border { side: 3, top: band(4), top_rows: 4, bottom: band(5), bottom_rows: 5, fill: [20, 20, 20] };
        // An EXIF block with nothing in it but an empty IFD0.
        let mut exif = b"Exif\0\0II\x2a\x00\x08\x00\x00\x00".to_vec();
        exif.extend([0u8; 6]);
        let meta = SourceMeta { exif: Some(exif), icc: None };
        let opts = Options { quality: 90, meta: &meta, keep_location: false, display_p3: false, border: Some(&border) };
        let dev = Developer::new(&Mask::default(), w, h, &Params::NONE);
        let jpeg = encode(Pixels { rgba: &rgba, width: w }, dev, w, h, &opts).unwrap();

        assert!(jpeg.windows(6).any(|s| s == b"Exif\0\0"), "EXIF kept");
        let img = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (out_w as u32, (h + 9) as u32));
        let near = |p: &image::Rgb<u8>, v: u8| p.0.iter().all(|&c| c.abs_diff(v) <= 3);
        assert!(near(img.get_pixel(0, 0), 20), "border");
        assert!(near(img.get_pixel(1, 20), 20), "side");
        assert!(near(img.get_pixel(20, 20), 128), "photo, untouched with no look");
    }
}
