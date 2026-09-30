//! Utilitas lepas-browser dari inti C++: persentil, lebar teks, gambar frame.

use std::os::raw::c_char;
use std::path::Path;

use super::ffi;
use crate::error::XhlError;

extern "C" {
    fn xhl_percentile(
        samples: *const u64,
        n: usize,
        p: f64,
        out: *mut f64,
        err: *mut *mut c_char,
    ) -> i32;
    fn xhl_text_width(utf8: *const c_char, out_cols: *mut u32, err: *mut *mut c_char) -> i32;
    fn xhl_draw_frame(
        utf8: *const c_char,
        path: *const c_char,
        scale: u32,
        err: *mut *mut c_char,
    ) -> i32;
}

/// Persentil dari sampel, interpolasi linear antar tetangga (`p` di `[0,1]`).
pub fn percentile(samples: &[u64], p: f64) -> Result<f64, XhlError> {
    let mut out: f64 = 0.0;
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: `samples` hidup selama panggilan; C++ menyalinnya sebelum mengurutkan.
    let rc = unsafe { xhl_percentile(samples.as_ptr(), samples.len(), p, &mut out, &mut err) };
    if rc != 0 {
        return Err(unsafe { ffi::map_error(rc, "percentile", err) });
    }
    Ok(out)
}

/// Lebar teks menurut font bitmap native (satuan kolom).
pub fn text_width(text: &str) -> Result<u32, XhlError> {
    let t = ffi::cstr(text)?;
    let mut cols: u32 = 0;
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: `t` hidup selama panggilan.
    let rc = unsafe { xhl_text_width(t.as_ptr(), &mut cols, &mut err) };
    if rc != 0 {
        return Err(unsafe { ffi::map_error(rc, "text_width", err) });
    }
    Ok(cols)
}

/// Gambar teks ke kanvas lalu tulis sebagai BMP 24-bit di `path`.
pub fn draw_frame(text: &str, path: &Path, scale: u32) -> Result<(), XhlError> {
    let t = ffi::cstr(text)?;
    let p = ffi::cstr(&path.to_string_lossy())?;
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: `t`/`p` hidup selama panggilan.
    let rc = unsafe { xhl_draw_frame(t.as_ptr(), p.as_ptr(), scale, &mut err) };
    if rc != 0 {
        return Err(unsafe { ffi::map_error(rc, "draw_frame", err) });
    }
    Ok(())
}
