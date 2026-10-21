//! WebAssembly entry points. The page allocates buffers in wasm memory with
//! `cs_alloc`, and keeps handles (pointers) to masks and developers:
//!
//! - a [`MaskBuilder`] takes a photo's rows strip by strip and becomes a
//!   [`Mask`], worked out once per photo;
//! - a [`Developer`] develops any region of the photo, at any size, in place;
//! - `cs_save` develops and encodes the whole photo, pulling its rows from
//!   the page through the imported `cs_pull_rows`.
//!
//! Settings come as an array of `PARAMS` floats, in the order of
//! [`params_from`].

use crate::jpeg::{encode, Border, Options, RowSource};
use crate::meta::{is_display_p3, read_jpeg};
use crate::{Developer, Mask, MaskBuilder, Params};

#[link(wasm_import_module = "env")]
extern "C" {
    /// Writes RGBA rows `y0..y0 + rows` of the photo being saved at `ptr`.
    fn cs_pull_rows(y0: u32, rows: u32, ptr: *mut u8);
}

const PARAMS: usize = 13;

/// # Safety
/// `ptr` must point to `PARAMS` floats.
unsafe fn params_from(ptr: *const f32, seed: u32) -> Params {
    let v = std::slice::from_raw_parts(ptr, PARAMS);
    Params {
        halation: v[0],
        halation_spread: v[1],
        grain: v[2],
        grain_size: v[3],
        tungsten: v[4],
        warmth: v[5],
        tint: v[6],
        contrast: v[7],
        fade: v[8],
        saturation: v[9],
        mono: v[10],
        vignette: v[11],
        leak: v[12],
        seed,
    }
}

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

#[no_mangle]
pub extern "C" fn cs_mask_new(width: u32, height: u32) -> *mut MaskBuilder {
    Box::into_raw(Box::new(MaskBuilder::new(width as usize, height as usize)))
}

/// # Safety
/// `b` from `cs_mask_new`; `ptr` points to `len` bytes of whole rows.
#[no_mangle]
pub unsafe extern "C" fn cs_mask_add(b: *mut MaskBuilder, ptr: *const u8, len: usize) {
    (*b).add_rows(std::slice::from_raw_parts(ptr, len));
}

/// Turns the builder into a mask; the builder is gone.
///
/// # Safety
/// `b` from `cs_mask_new`.
#[no_mangle]
pub unsafe extern "C" fn cs_mask_finish(b: *mut MaskBuilder) -> *mut Mask {
    Box::into_raw(Box::new(Box::from_raw(b).finish()))
}

/// A mask with no highlights, for when there's no time to pool any.
#[no_mangle]
pub extern "C" fn cs_mask_empty() -> *mut Mask {
    Box::into_raw(Box::new(Mask::default()))
}

/// # Safety
/// `m` from `cs_mask_finish` or `cs_mask_empty`.
#[no_mangle]
pub unsafe extern "C" fn cs_mask_free(m: *mut Mask) {
    drop(Box::from_raw(m));
}

/// # Safety
/// `mask` from `cs_mask_finish` or `cs_mask_empty`; `params` points to `PARAMS` floats.
#[no_mangle]
pub unsafe extern "C" fn cs_dev_new(mask: *const Mask, width: u32, height: u32, params: *const f32, seed: u32) -> *mut Developer {
    let p = params_from(params, seed);
    Box::into_raw(Box::new(Developer::new(&*mask, width as usize, height as usize, &p)))
}

/// # Safety
/// `dev` from `cs_dev_new`; `ptr` points to `rw * rh * 4` bytes.
#[no_mangle]
pub unsafe extern "C" fn cs_dev_develop(dev: *mut Developer, ptr: *mut u8, x0: u32, y0: u32, rw: u32, rh: u32) {
    let len = rw as usize * rh as usize * 4;
    (*dev).develop(std::slice::from_raw_parts_mut(ptr, len), x0 as usize, y0 as usize, rw as usize, rh as usize);
}

/// # Safety
/// `dev` from `cs_dev_new`.
#[no_mangle]
pub unsafe extern "C" fn cs_dev_free(dev: *mut Developer) {
    drop(Box::from_raw(dev));
}

/// What the start of a file holds: 1 = EXIF, 2 = a Display P3 profile.
///
/// # Safety
/// `ptr` points to `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn cs_meta_info(ptr: *const u8, len: usize) -> u32 {
    let meta = read_jpeg(std::slice::from_raw_parts(ptr, len));
    meta.exif.is_some() as u32 | (meta.icc.as_deref().is_some_and(is_display_p3) as u32) << 1
}

/// The result of `cs_save`: the JPEG, or why there is none.
pub struct Saved {
    ok: bool,
    bytes: Vec<u8>,
}

/// Rows of the photo being saved, from the page.
struct Pull {
    width: usize,
}

impl RowSource for Pull {
    fn rows(&mut self, y0: usize, out: &mut [u8]) {
        let rows = out.len() / (self.width * 4);
        unsafe { cs_pull_rows(y0 as u32, rows as u32, out.as_mut_ptr()) };
    }
}

/// Develops and encodes a `width`×`height` photo, pulling its rows from the
/// page. `meta` is the start of the original file (or empty); the border
/// bands, when `side` or their rows aren't zero, are RGBA at the full width.
///
/// # Safety
/// Pointers from `cs_alloc` (or handles from `cs_mask_*`) with the lengths given.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn cs_save(
    mask: *const Mask,
    width: u32,
    height: u32,
    params: *const f32,
    seed: u32,
    quality: u32,
    meta_ptr: *const u8,
    meta_len: usize,
    keep_location: u32,
    display_p3: u32,
    side: u32,
    top_ptr: *const u8,
    top_rows: u32,
    bottom_ptr: *const u8,
    bottom_rows: u32,
    fill: u32,
) -> *mut Saved {
    let (w, h) = (width as usize, height as usize);
    let p = params_from(params, seed);
    let meta = if meta_len > 0 { read_jpeg(std::slice::from_raw_parts(meta_ptr, meta_len)) } else { Default::default() };
    let out_w = w + 2 * side as usize;
    let band = |ptr: *const u8, rows: u32| {
        if rows == 0 { Vec::new() } else { std::slice::from_raw_parts(ptr, out_w * rows as usize * 4).to_vec() }
    };
    let border = Border {
        side: side as usize,
        top: band(top_ptr, top_rows),
        top_rows: top_rows as usize,
        bottom: band(bottom_ptr, bottom_rows),
        bottom_rows: bottom_rows as usize,
        fill: [(fill >> 16) as u8, (fill >> 8) as u8, fill as u8],
    };
    let has_border = side > 0 || top_rows > 0 || bottom_rows > 0;
    let opts = Options {
        quality: quality as u8,
        meta: &meta,
        keep_location: keep_location != 0,
        display_p3: display_p3 != 0,
        border: has_border.then_some(&border),
    };
    let dev = Developer::new(&*mask, w, h, &p);
    let saved = match encode(Pull { width: w }, dev, w, h, &opts) {
        Ok(bytes) => Saved { ok: true, bytes },
        Err(e) => Saved { ok: false, bytes: e.into_bytes() },
    };
    Box::into_raw(Box::new(saved))
}

/// # Safety
/// `s` from `cs_save`.
#[no_mangle]
pub unsafe extern "C" fn cs_saved_ok(s: *const Saved) -> u32 {
    (*s).ok as u32
}

/// # Safety
/// `s` from `cs_save`.
#[no_mangle]
pub unsafe extern "C" fn cs_saved_ptr(s: *const Saved) -> *const u8 {
    (*s).bytes.as_ptr()
}

/// # Safety
/// `s` from `cs_save`.
#[no_mangle]
pub unsafe extern "C" fn cs_saved_len(s: *const Saved) -> usize {
    (*s).bytes.len()
}

/// # Safety
/// `s` from `cs_save`.
#[no_mangle]
pub unsafe extern "C" fn cs_saved_free(s: *mut Saved) {
    drop(Box::from_raw(s));
}
