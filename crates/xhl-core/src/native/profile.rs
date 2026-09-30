//! Profil penyamaran (skema bertipe + validasi) dan gambar frame BMP.

use std::os::raw::c_char;
use std::path::Path;

use super::ffi;
use crate::error::XhlError;

extern "C" {
    fn xhl_profile_validate(json: *const c_char, err: *mut *mut c_char) -> i32;
    fn xhl_profile_from_json(json: *const c_char, err: *mut *mut c_char) -> i32;
    fn xhl_profile_clear();
    fn xhl_profile_dump(out_json: *mut *mut c_char, err: *mut *mut c_char) -> i32;
    fn xhl_profile_loaded() -> i32;
}

/// Pegangan profil proses. `Drop` membersihkan state C++ agar tidak ada profil
/// yang terbawa antar-perintah di proses yang sama.
#[derive(Debug)]
pub struct ProfileHandle {
    _priv: (),
}

impl ProfileHandle {
    /// Muat dari berkas JSON. Berkas tidak ada / JSON tidak sah → error yang
    /// menyebut path/penyebab.
    pub fn from_file(path: &Path) -> Result<Self, XhlError> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            XhlError::Invalid(format!(
                "profil tidak dapat dibaca ({}): {e}",
                path.display()
            ))
        })?;
        Self::from_json(&raw)
    }

    /// Muat dari JSON inline (mis. isi kunci `profile` di `XHL_CONFIG`).
    pub fn from_json(json: &str) -> Result<Self, XhlError> {
        let j = ffi::cstr(json)?;
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: j hidup selama panggilan; err ditulis C++.
        let rc = unsafe { xhl_profile_from_json(j.as_ptr(), &mut err) };
        if rc != 0 {
            // SAFETY: err dari C++.
            return Err(unsafe { ffi::map_error(rc, "profil", err) });
        }
        Ok(Self { _priv: () })
    }

    /// Validasi saja, tanpa menyimpan.
    pub fn validate(json: &str) -> Result<(), XhlError> {
        let j = ffi::cstr(json)?;
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: j hidup selama panggilan.
        let rc = unsafe { xhl_profile_validate(j.as_ptr(), &mut err) };
        if rc != 0 {
            return Err(unsafe { ffi::map_error(rc, "profil tidak sah", err) });
        }
        Ok(())
    }

    /// Apakah ada profil tersimpan di state C++?
    pub fn is_loaded() -> bool {
        // SAFETY: tidak ada argumen; hanya membaca flag.
        unsafe { xhl_profile_loaded() == 1 }
    }

    /// Serialisasi balik. Hasilnya wajib lolos `validate` lagi (bolak-balik).
    pub fn dump(&self) -> Result<String, XhlError> {
        let mut out: *mut c_char = std::ptr::null_mut();
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: out dialokasikan C++.
        let rc = unsafe { xhl_profile_dump(&mut out, &mut err) };
        if rc != 0 {
            return Err(unsafe { ffi::map_error(rc, "profil", err) });
        }
        // SAFETY: out dari C++.
        Ok(unsafe { ffi::take_string(out) })
    }
}

impl Drop for ProfileHandle {
    fn drop(&mut self) {
        // SAFETY: tidak ada argumen; membersihkan state proses.
        unsafe { xhl_profile_clear() };
    }
}
