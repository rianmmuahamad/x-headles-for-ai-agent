//! Deklarasi FFI mentah + helper aman untuk memindahkan memori lintas batas.
//!
//! Modul ini adalah satu-satunya tempat `unsafe` boleh muncul di jalur native:
//! submodul di bawahnya membungkus pemanggilan dengan guard yang membebaskan
//! memori C++.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use crate::error::XhlError;

/// Kunci tidak ada (accessor config).
pub const MISSING: i32 = -1;
/// Kunci ada tetapi tipenya salah (accessor config).
pub const TYPE: i32 = -2;

extern "C" {
    // entry.cpp
    fn xhl_native_version() -> *const c_char;
    pub fn xhl_string_free(s: *mut c_char);
    pub fn xhl_strings_free(arr: *mut *mut c_char, n: usize);
}

/// Versi inti native. String statis di C++: tidak dibebaskan.
pub fn version() -> &'static str {
    // SAFETY: C++ mengembalikan pointer ke literal statis yang hidup selama
    // program berjalan.
    unsafe {
        let p = xhl_native_version();
        if p.is_null() {
            return "xhl-native/unknown";
        }
        CStr::from_ptr(p).to_str().unwrap_or("xhl-native/unknown")
    }
}

/// String Rust -> C. Gagal bila memuat NUL interior.
pub fn cstr(s: &str) -> Result<CString, XhlError> {
    CString::new(s).map_err(|_| XhlError::Internal(format!("teks memuat NUL: {s:?}")))
}

/// Baca string C++ **tanpa** membebaskan apa pun.
///
/// # Safety
/// `p` harus null atau string C++ xhl yang masih hidup.
pub unsafe fn read_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // from_utf8_lossy dipakai agar byte rusak tidak membuat proses kehilangan
    // pesan error yang justru dibutuhkan untuk diagnosis.
    CStr::from_ptr(p).to_string_lossy().into_owned()
}

/// Ambil kepemilikan satu string keluaran C++ (UTF-8) lalu bebaskan.
///
/// # Safety
/// `p` harus berasal dari fungsi C++ xhl (dialokasikan `malloc`) atau null,
/// dan belum pernah dibebaskan.
pub unsafe fn take_string(p: *mut c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    let out = read_string(p);
    xhl_string_free(p);
    out
}

/// Ambil kepemilikan daftar string keluaran C++ lalu bebaskan seluruhnya.
///
/// Elemen **tidak** dibebaskan satu per satu: `xhl_strings_free` sudah
/// membebaskan tiap elemen beserta lariknya. Membebaskan keduanya adalah
/// double free.
///
/// # Safety
/// `arr`/`n` harus berasal dari fungsi C++ xhl dan belum pernah dibebaskan.
pub unsafe fn take_strings(arr: *mut *mut c_char, n: usize) -> Vec<String> {
    if arr.is_null() || n == 0 {
        if !arr.is_null() {
            xhl_strings_free(arr, n);
        }
        return Vec::new();
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push(read_string(*arr.add(i)));
    }
    xhl_strings_free(arr, n);
    out
}

/// Pesan error C++ (bila ada) menjadi `XhlError`. `context` muncul lebih dulu
/// agar pemanggil tahu tahap mana yang gagal; kode native disertakan bila C++
/// tidak mengisi pesan — kondisi yang menandakan bug, bukan masukan pengguna.
///
/// # Safety
/// `err` harus null atau string C++ xhl yang belum dibebaskan.
pub unsafe fn map_error(code: i32, context: &str, err: *mut c_char) -> XhlError {
    let msg = take_string(err);
    if msg.is_empty() {
        XhlError::Config(format!("{context} (kode native {code})"))
    } else {
        XhlError::Config(format!("{context}: {msg}"))
    }
}
