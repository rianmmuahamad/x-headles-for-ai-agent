//! Pembungkus aman untuk config native (adaptasi pola `MaskConfig` camoufox).
//!
//! Nilai dikembalikan sebagai `Option` (kunci absen → `None`) atau
//! `XhlError::Config` (tipe salah, pesannya menyebut nama kunci).

use std::os::raw::c_char;

use super::ffi::{self, MISSING};
use crate::error::XhlError;

/// Prefix env var. `XHL_CONFIG_1..N` disambung, lalu `XHL_CONFIG`.
pub const PREFIX: &str = "XHL_CONFIG";

extern "C" {
    fn xhl_config_load(prefix: *const c_char, err: *mut *mut c_char) -> i32;
    fn xhl_config_u32(key: *const c_char, out: *mut u32, err: *mut *mut c_char) -> i32;
    fn xhl_config_bool(key: *const c_char, out: *mut i32, err: *mut *mut c_char) -> i32;
    fn xhl_config_f64(key: *const c_char, out: *mut f64, err: *mut *mut c_char) -> i32;
    fn xhl_config_string(key: *const c_char, out: *mut *mut c_char, err: *mut *mut c_char) -> i32;
    fn xhl_config_string_list(
        key: *const c_char,
        out: *mut *mut *mut c_char,
        n: *mut usize,
        err: *mut *mut c_char,
    ) -> i32;
    fn xhl_config_rect(
        kx: *const c_char,
        ky: *const c_char,
        kw: *const c_char,
        kh: *const c_char,
        out: *mut [u32; 4],
        err: *mut *mut c_char,
    ) -> i32;
    fn xhl_config_dump(out_json: *mut *mut c_char, err: *mut *mut c_char) -> i32;
    fn xhl_config_known_keys(
        out: *mut *mut *mut c_char,
        n: *mut usize,
        err: *mut *mut c_char,
    ) -> i32;
    fn xhl_config_load_error(out: *mut *mut c_char, err: *mut *mut c_char) -> i32;
    fn xhl_config_validate(
        bad_keys: *mut *mut *mut c_char,
        n: *mut usize,
        err: *mut *mut c_char,
    ) -> i32;
}

/// Config dari environment, diparse dan divalidasi sekali per proses oleh C++.
///
/// `load` bersifat idempoten: pemanggilan berikutnya tidak membaca ulang env.
/// Tanpa `XHL_CONFIG*`, seluruh accessor mengembalikan `None` dan pemanggil
/// memakai default Rust — perilaku lama tidak berubah.
#[derive(Debug)]
pub struct NativeConfig {
    _priv: (),
}

impl NativeConfig {
    /// Muat config. Gagal hanya bila JSON tidak sah (atau bukan objek).
    ///
    /// JSON tidak sah **bukan** kegagalan fatal di C++ (config kosong), tetapi
    /// di sini dilaporkan: menjalankan xhl dengan config yang diam-diam diabaikan
    /// lebih membingungkan daripada gagal dengan sebab yang jelas.
    pub fn load() -> Result<Self, XhlError> {
        let prefix = ffi::cstr(PREFIX)?;
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: prefix hidup selama panggilan; err hanya ditulis C++.
        let rc = unsafe { xhl_config_load(prefix.as_ptr(), &mut err) };
        if rc == 0 {
            return Ok(Self { _priv: () });
        }
        // SAFETY: err berasal dari C++ dan belum dibebaskan.
        Err(unsafe { ffi::map_error(rc, "config native", err) })
    }

    fn u32(&self, key: &str) -> Result<Option<u32>, XhlError> {
        let k = ffi::cstr(key)?;
        let mut out: u32 = 0;
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: k hidup selama panggilan; out/err hanya ditulis C++.
        let rc = unsafe { xhl_config_u32(k.as_ptr(), &mut out, &mut err) };
        match rc {
            0 => Ok(Some(out)),
            MISSING => Ok(None),
            _ => Err(unsafe { ffi::map_error(rc, "config native", err) }),
        }
    }

    fn bool(&self, key: &str) -> Result<Option<bool>, XhlError> {
        let k = ffi::cstr(key)?;
        let mut out: i32 = 0;
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: k hidup selama panggilan.
        let rc = unsafe { xhl_config_bool(k.as_ptr(), &mut out, &mut err) };
        match rc {
            0 => Ok(Some(out != 0)),
            MISSING => Ok(None),
            _ => Err(unsafe { ffi::map_error(rc, "config native", err) }),
        }
    }

    /// Nilai `key` sebagai `f64`.
    pub fn f64(&self, key: &str) -> Result<Option<f64>, XhlError> {
        let k = ffi::cstr(key)?;
        let mut out: f64 = 0.0;
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: k hidup selama panggilan.
        let rc = unsafe { xhl_config_f64(k.as_ptr(), &mut out, &mut err) };
        match rc {
            0 => Ok(Some(out)),
            MISSING => Ok(None),
            _ => Err(unsafe { ffi::map_error(rc, "config native", err) }),
        }
    }

    fn string(&self, key: &str) -> Result<Option<String>, XhlError> {
        let k = ffi::cstr(key)?;
        let mut out: *mut c_char = std::ptr::null_mut();
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: k hidup selama panggilan; out dialokasikan C++.
        let rc = unsafe { xhl_config_string(k.as_ptr(), &mut out, &mut err) };
        match rc {
            0 => {
                // SAFETY: out dari C++.
                Ok(Some(unsafe { ffi::take_string(out) }))
            }
            MISSING => Ok(None),
            _ => Err(unsafe { ffi::map_error(rc, "config native", err) }),
        }
    }

    /// Nilai `key` sebagai daftar string.
    pub fn string_list(&self, key: &str) -> Result<Vec<String>, XhlError> {
        let k = ffi::cstr(key)?;
        let mut out: *mut *mut c_char = std::ptr::null_mut();
        let mut n: usize = 0;
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: k hidup selama panggilan; out dialokasikan C++.
        let rc = unsafe { xhl_config_string_list(k.as_ptr(), &mut out, &mut n, &mut err) };
        match rc {
            0 => {
                // SAFETY: out/n dari C++.
                Ok(unsafe { ffi::take_strings(out, n) })
            }
            MISSING => Ok(Vec::new()),
            _ => Err(unsafe { ffi::map_error(rc, "config native", err) }),
        }
    }

    fn rect(&self, keys: [&str; 4]) -> Result<Option<[u32; 4]>, XhlError> {
        let [kx, ky, kw, kh] = keys;
        let (kx, ky, kw, kh) = (
            ffi::cstr(kx)?,
            ffi::cstr(ky)?,
            ffi::cstr(kw)?,
            ffi::cstr(kh)?,
        );
        let mut out: [u32; 4] = [0; 4];
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: semua pointer hidup selama panggilan.
        let rc = unsafe {
            xhl_config_rect(
                kx.as_ptr(),
                ky.as_ptr(),
                kw.as_ptr(),
                kh.as_ptr(),
                &mut out,
                &mut err,
            )
        };
        match rc {
            0 => Ok(Some(out)),
            MISSING => Ok(None),
            _ => Err(unsafe { ffi::map_error(rc, "config native", err) }),
        }
    }

    /// Nilai `draw_x..draw_h` sebagai rect. `None` bila keempatnya absen.
    pub fn draw_rect(&self) -> Result<Option<[u32; 4]>, XhlError> {
        self.rect(["draw_x", "draw_y", "draw_w", "draw_h"])
    }

    /// Seluruh config tervalidasi sebagai JSON (untuk `xhl native config`).
    pub fn dump(&self) -> Result<String, XhlError> {
        let mut out: *mut c_char = std::ptr::null_mut();
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: out dialokasikan C++.
        let rc = unsafe { xhl_config_dump(&mut out, &mut err) };
        if rc != 0 {
            return Err(unsafe { ffi::map_error(rc, "config native", err) });
        }
        // SAFETY: out dari C++.
        Ok(unsafe { ffi::take_string(out) })
    }

    /// Nama kunci config yang dikenal, tanpa nilainya.
    pub fn known_keys(&self) -> Result<Vec<String>, XhlError> {
        let mut out: *mut *mut c_char = std::ptr::null_mut();
        let mut n: usize = 0;
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: out dialokasikan C++.
        let rc = unsafe { xhl_config_known_keys(&mut out, &mut n, &mut err) };
        if rc != 0 {
            return Err(unsafe { ffi::map_error(rc, "config native", err) });
        }
        // SAFETY: out/n dari C++.
        Ok(unsafe { ffi::take_strings(out, n) })
    }

    /// Kunci yang ada tetapi tipenya salah. Daftar tidak kosong berarti
    /// konfigurasi akan diam-diam diabaikan saat dipakai — itu harus dilaporkan
    /// sebelum perilakunya salah.
    pub fn validate_types(&self) -> Result<Vec<String>, XhlError> {
        let mut out: *mut *mut c_char = std::ptr::null_mut();
        let mut n: usize = 0;
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: out dialokasikan C++.
        let rc = unsafe { xhl_config_validate(&mut out, &mut n, &mut err) };
        if rc != 0 {
            return Err(unsafe { ffi::map_error(rc, "config native", err) });
        }
        // SAFETY: out/n dari C++.
        Ok(unsafe { ffi::take_strings(out, n) })
    }

    /// Pesan kegagalan penguraian, bila ada (JSON tidak sah / bukan objek).
    pub fn load_error(&self) -> Option<String> {
        let mut out: *mut c_char = std::ptr::null_mut();
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: out dialokasikan C++.
        let rc = unsafe { xhl_config_load_error(&mut out, &mut err) };
        if rc != 0 {
            // SAFETY: err dari C++.
            let _ = unsafe { ffi::take_string(err) };
            return None;
        }
        // SAFETY: out dari C++.
        Some(unsafe { ffi::take_string(out) })
    }
}

// --- Accessor bertipe yang dipakai crate lain. ---

impl NativeConfig {
    pub fn http_profile(&self) -> Option<String> {
        self.string("http_profile").ok().flatten()
    }
    pub fn no_wait(&self) -> bool {
        self.bool("no_wait").ok().flatten().unwrap_or(false)
    }
    pub fn llm_model(&self) -> Option<String> {
        self.string("llm_model").ok().flatten()
    }
    pub fn query_ids_path(&self) -> Option<String> {
        self.string("query_ids_path").ok().flatten()
    }
    pub fn emulation_profile(&self) -> Option<String> {
        self.string("emulation_profile").ok().flatten()
    }
    pub fn profile_source(&self) -> Option<String> {
        self.string("profile").ok().flatten()
    }
    pub fn content_max_bytes(&self) -> Option<u64> {
        self.u32("content_max_bytes").ok().flatten().map(u64::from)
    }
    pub fn retry_attempts(&self) -> Option<usize> {
        self.u32("retry_attempts")
            .ok()
            .flatten()
            .map(|v| v as usize)
    }
    pub fn retry_backoff_ms(&self) -> Option<u64> {
        self.u32("retry_backoff_ms").ok().flatten().map(u64::from)
    }
    pub fn request_timeout_secs(&self) -> Option<u64> {
        self.u32("request_timeout_secs")
            .ok()
            .flatten()
            .map(u64::from)
    }
    pub fn discovery_concurrency(&self) -> Option<usize> {
        self.u32("discovery_concurrency")
            .ok()
            .flatten()
            .map(|v| v as usize)
    }
    pub fn media_chunk_bytes(&self) -> Option<usize> {
        self.u32("media_chunk_bytes")
            .ok()
            .flatten()
            .map(|v| v as usize)
    }
    /// Nilai string mentah untuk kunci apa pun.
    pub fn raw_string(&self, key: &str) -> Option<String> {
        self.string(key).ok().flatten()
    }
    pub fn raw_bool(&self, key: &str) -> Option<bool> {
        self.bool(key).ok().flatten()
    }
    pub fn raw_u32(&self, key: &str) -> Option<u32> {
        self.u32(key).ok().flatten()
    }
    /// Tipe salah dilaporkan, bukan ditelan.
    pub fn checked_u32(&self, key: &str) -> Result<Option<u32>, XhlError> {
        self.u32(key)
    }
    pub fn checked_bool(&self, key: &str) -> Result<Option<bool>, XhlError> {
        self.bool(key)
    }
    pub fn checked_string(&self, key: &str) -> Result<Option<String>, XhlError> {
        self.string(key)
    }
}

/// Nilai default yang harus sama dengan perilaku xhl sebelum inti native ada.
pub mod defaults {
    pub const RETRY_ATTEMPTS: usize = 3;
    pub const RETRY_BACKOFF_MS: u64 = 400;
    pub const REQUEST_TIMEOUT_SECS: u64 = 30;
    pub const DISCOVERY_CONCURRENCY: usize = 8;
    pub const MEDIA_CHUNK_BYTES: usize = 5 * 1024 * 1024;
    pub const CONTENT_MAX_BYTES: u64 = 1_048_576;
    pub const HTTP_PROFILE: &str = "firefox_133";
    pub const LLM_MODEL: &str = "gpt-4o-mini";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cocok_dengan_konstanta_lama() {
        // Nilai ini harus sama dengan konstanta yang dulu hardcoded; kalau
        // berubah, perilaku xhl tanpa `XHL_CONFIG` ikut berubah.
        assert_eq!(defaults::RETRY_ATTEMPTS, 3);
        assert_eq!(defaults::RETRY_BACKOFF_MS, 400);
        assert_eq!(defaults::REQUEST_TIMEOUT_SECS, 30);
        assert_eq!(defaults::DISCOVERY_CONCURRENCY, 8);
        assert_eq!(defaults::MEDIA_CHUNK_BYTES, 5 * 1024 * 1024);
        assert_eq!(defaults::CONTENT_MAX_BYTES, 1_048_576);
        assert_eq!(defaults::HTTP_PROFILE, "firefox_133");
    }
}
