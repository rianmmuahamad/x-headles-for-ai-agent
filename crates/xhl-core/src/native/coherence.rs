//! Koherensi identitas + derivasi header (adaptasi `coherence.py` camoufox).

use std::os::raw::c_char;

use super::ffi;
use crate::error::XhlError;

extern "C" {
    fn xhl_coherence_validate(
        profile_json: *const c_char,
        target_os: *const c_char,
        out: *mut *mut RawViolation,
        n: *mut usize,
        err: *mut *mut c_char,
    ) -> i32;
    fn xhl_coherence_apply(
        profile_json: *const c_char,
        target_os: *const c_char,
        out_json: *mut *mut c_char,
        out: *mut *mut RawViolation,
        n: *mut usize,
        err: *mut *mut c_char,
    ) -> i32;
    fn xhl_coherence_drop_incoherent(
        profile_json: *const c_char,
        target_os: *const c_char,
        out_json: *mut *mut c_char,
        out: *mut *mut RawViolation,
        n: *mut usize,
        err: *mut *mut c_char,
    ) -> i32;
    fn xhl_violations_free(v: *mut RawViolation, n: usize);

    fn xhl_headers_from_profile(
        profile_json: *const c_char,
        out: *mut RawHeaders,
        err: *mut *mut c_char,
    ) -> i32;
    fn xhl_headers_free(h: *mut RawHeaders);
}

#[repr(C)]
struct RawViolation {
    rule: *const c_char,
    detail: *const c_char,
}

/// Satu pelanggaran aturan koherensi. Nama `rule` sama persis dengan yang ada
/// di `coherence.py`, sehingga hasilnya dapat dibandingkan baris-per-baris.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Violation {
    pub rule: String,
    pub detail: String,
}

/// Pelanggaran yang tersisa beserta JSON hasil perbaikan/penyaringan.
#[derive(Debug, Clone)]
pub struct JsonReport {
    pub violations: Vec<Violation>,
    pub json: String,
}

/// Header HTTP hasil derivasi profil.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderOverrides {
    pub user_agent: Option<String>,
    pub accept_language: Option<String>,
    pub accept_encoding: Option<String>,
    pub accept: Option<String>,
    pub dnt: Option<String>,
    pub viewport_width: Option<String>,
    pub sec_ch_ua: Option<String>,
    pub sec_ch_ua_mobile: Option<String>,
    pub sec_ch_ua_platform: Option<String>,
    pub order: Vec<String>,
    /// True bila profil benar-benar menyumbang sesuatu; false = pakai bawaan.
    pub from_profile: bool,
    /// True bila `user_agent` berasal dari `navigator.userAgent` (cadangan).
    pub user_agent_from_navigator: bool,
}

#[repr(C)]
struct RawHeaders {
    user_agent: *const c_char,
    accept_language: *const c_char,
    accept_encoding: *const c_char,
    accept: *const c_char,
    dnt: *const c_char,
    viewport_width: *const c_char,
    sec_ch_ua: *const c_char,
    sec_ch_ua_mobile: *const c_char,
    sec_ch_ua_platform: *const c_char,
    order: *const *const c_char,
    order_n: usize,
    from_profile: i32,
    from_navigator_fallback: i32,
}

/// Pindahkan larik pelanggaran dari C++ ke `Vec`, lalu bebaskan di C++.
///
/// # Safety
/// `arr`/`n` harus berasal dari fungsi koherensi xhl dan belum dibebaskan.
unsafe fn take_violations(arr: *mut RawViolation, n: usize) -> Vec<Violation> {
    if arr.is_null() || n == 0 {
        if !arr.is_null() {
            xhl_violations_free(arr, n);
        }
        return Vec::new();
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let v = &*arr.add(i);
        out.push(Violation {
            rule: ffi::read_string(v.rule),
            detail: ffi::read_string(v.detail),
        });
    }
    xhl_violations_free(arr, n);
    out
}

/// Periksa koherensi profil untuk `target_os` (`"win"`/`"mac"`/`"lin"`).
/// Daftar kosong berarti identitas koheren.
pub fn check_profile(profile_json: &str, target_os: &str) -> Result<Vec<Violation>, XhlError> {
    let j = ffi::cstr(profile_json)?;
    let os = ffi::cstr(target_os)?;
    let mut out: *mut RawViolation = std::ptr::null_mut();
    let mut n: usize = 0;
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: j/os hidup selama panggilan; out dialokasikan C++.
    let rc = unsafe { xhl_coherence_validate(j.as_ptr(), os.as_ptr(), &mut out, &mut n, &mut err) };
    if rc != 0 {
        return Err(unsafe { ffi::map_error(rc, "koherensi", err) });
    }
    // SAFETY: out/n dari C++.
    Ok(unsafe { take_violations(out, n) })
}

/// Perbaiki apa yang dapat ditentukan; laporkan yang tersisa.
pub fn apply_coherence(profile_json: &str, target_os: &str) -> Result<JsonReport, XhlError> {
    let j = ffi::cstr(profile_json)?;
    let os = ffi::cstr(target_os)?;
    let mut out_json: *mut c_char = std::ptr::null_mut();
    let mut out: *mut RawViolation = std::ptr::null_mut();
    let mut n: usize = 0;
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: pointer hidup selama panggilan.
    let rc = unsafe {
        xhl_coherence_apply(
            j.as_ptr(),
            os.as_ptr(),
            &mut out_json,
            &mut out,
            &mut n,
            &mut err,
        )
    };
    if rc != 0 {
        return Err(unsafe { ffi::map_error(rc, "koherensi apply", err) });
    }
    // SAFETY: out/n dan out_json dari C++.
    let violations = unsafe { take_violations(out, n) };
    let json = unsafe { ffi::take_string(out_json) };
    Ok(JsonReport { violations, json })
}

/// Buang nilai yang tidak dapat dipertahankan identitas (mis. GPU asing).
pub fn drop_incoherent(profile_json: &str, target_os: &str) -> Result<JsonReport, XhlError> {
    let j = ffi::cstr(profile_json)?;
    let os = ffi::cstr(target_os)?;
    let mut out_json: *mut c_char = std::ptr::null_mut();
    let mut out: *mut RawViolation = std::ptr::null_mut();
    let mut n: usize = 0;
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: pointer hidup selama panggilan.
    let rc = unsafe {
        xhl_coherence_drop_incoherent(
            j.as_ptr(),
            os.as_ptr(),
            &mut out_json,
            &mut out,
            &mut n,
            &mut err,
        )
    };
    if rc != 0 {
        return Err(unsafe { ffi::map_error(rc, "koherensi drop", err) });
    }
    // SAFETY: out/n dan out_json dari C++.
    let violations = unsafe { take_violations(out, n) };
    let json = unsafe { ffi::take_string(out_json) };
    Ok(JsonReport { violations, json })
}

/// Derivasikan header HTTP dari profil. `None` bila profil tidak menyumbang
/// apa pun (semua kunci header absen) — pemanggil memakai header bawaan xhl.
pub fn headers_from_profile(profile_json: &str) -> Result<Option<HeaderOverrides>, XhlError> {
    let j = ffi::cstr(profile_json)?;
    let mut raw = RawHeaders {
        user_agent: std::ptr::null(),
        accept_language: std::ptr::null(),
        accept_encoding: std::ptr::null(),
        accept: std::ptr::null(),
        dnt: std::ptr::null(),
        viewport_width: std::ptr::null(),
        sec_ch_ua: std::ptr::null(),
        sec_ch_ua_mobile: std::ptr::null(),
        sec_ch_ua_platform: std::ptr::null(),
        order: std::ptr::null(),
        order_n: 0,
        from_profile: 0,
        from_navigator_fallback: 0,
    };
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: j hidup selama panggilan; raw seluruhnya ditulis C++.
    let rc = unsafe { xhl_headers_from_profile(j.as_ptr(), &mut raw, &mut err) };
    if rc != 0 {
        return Err(unsafe { ffi::map_error(rc, "headers profil", err) });
    }

    // SAFETY: seluruh pointer dalam raw berasal dari C++ dan dibebaskan sekali
    // di akhir lewat xhl_headers_free.
    let result = unsafe {
        let take = |p: *const c_char| -> Option<String> {
            if p.is_null() {
                None
            } else {
                Some(ffi::read_string(p))
            }
        };
        let mut order = Vec::with_capacity(raw.order_n);
        for i in 0..raw.order_n {
            order.push(ffi::read_string(*raw.order.add(i)));
        }
        let out = HeaderOverrides {
            user_agent: take(raw.user_agent),
            accept_language: take(raw.accept_language),
            accept_encoding: take(raw.accept_encoding),
            accept: take(raw.accept),
            dnt: take(raw.dnt),
            viewport_width: take(raw.viewport_width),
            sec_ch_ua: take(raw.sec_ch_ua),
            sec_ch_ua_mobile: take(raw.sec_ch_ua_mobile),
            sec_ch_ua_platform: take(raw.sec_ch_ua_platform),
            order,
            from_profile: raw.from_profile == 1,
            user_agent_from_navigator: raw.from_navigator_fallback == 1,
        };
        xhl_headers_free(&mut raw);
        out
    };

    if result.from_profile {
        Ok(Some(result))
    } else {
        Ok(None)
    }
}
