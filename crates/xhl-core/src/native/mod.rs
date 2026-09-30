//! Jembatan ke inti C++ (`crates/xhl-core/native/`).
//!
//! Seluruh `unsafe` FFI terkurung di modul ini — submodul di bawahnya memakai
//! API aman yang membebaskan memori C++ lewat guard `Drop`.
//!
//! Invarian yang berlaku untuk setiap fungsi C++:
//!  - Kembalian `int32_t`: `0` sukses, negatif = accessor config
//!    (`-1` kunci tidak ada, `-2` tipe salah), positif = kegagalan nyata.
//!  - String keluar selalu UTF-8 yang dialokasikan C++ (`malloc`) dan wajib
//!    dibebaskan lewat [`ffi::take_string`]/[`ffi::take_strings`].
//!  - Tidak ada `std::string` yang menyeberang batas.

pub mod ffi;

pub mod coherence;
pub mod config;
pub mod profile;
pub mod util;

pub use coherence::{
    apply_coherence, check_profile, drop_incoherent, HeaderOverrides, JsonReport, Violation,
};
pub use config::NativeConfig;
pub use profile::ProfileHandle;

use std::sync::LazyLock;

/// Config native proses, dimuat sekali.
///
/// Dipakai jalur yang tidak menerima [`crate::Config`] (helper upload media,
/// discovery, transport) supaya kunci yang sama mengatur semua tempat. `None`
/// berarti config tidak dapat dimuat (JSON tidak sah) — pemanggil memakai
/// default, dan `xhl doctor`/`xhl native config` yang melaporkan sebabnya.
static RUNTIME: LazyLock<Option<NativeConfig>> = LazyLock::new(|| NativeConfig::load().ok());

pub fn runtime() -> Option<&'static NativeConfig> {
    RUNTIME.as_ref()
}

/// Nilai `key` sebagai usize, atau `default` bila kunci absen/tidak dapat dimuat.
pub fn config_usize(key: &str, default: usize) -> usize {
    runtime()
        .and_then(|c| c.checked_u32(key).ok().flatten())
        .map(|v| v as usize)
        .unwrap_or(default)
}

/// Nilai `key` sebagai u64, atau `default`.
pub fn config_u64(key: &str, default: u64) -> u64 {
    runtime()
        .and_then(|c| c.checked_u32(key).ok().flatten())
        .map(u64::from)
        .unwrap_or(default)
}

/// Nilai `key` sebagai string, atau `default`.
pub fn config_string(key: &str, default: &str) -> String {
    runtime()
        .and_then(|c| c.checked_string(key).ok().flatten())
        .unwrap_or_else(|| default.to_owned())
}

/// Header HTTP turunan profil anti-detect, dimuat sekali per proses.
///
/// Profil diambil dari kunci `profile`: bila nilainya menunjuk berkas yang ada,
/// berkas itu dibaca; selain itu nilainya dianggap JSON inline. Profil yang
/// tidak dapat dibaca/tidak sah **tidak** menggagalkan permintaan — xhl kembali
/// memakai header bawaannya, dan `xhl anticheck --profile` yang melaporkan
/// sebabnya.
///
/// Di-cache: `build()` memanggil ini per permintaan, dan membaca berkas tiap
/// kali akan mengubah satu permintaan menjadi I/O disk.
static HEADER_OVERRIDES: LazyLock<Option<HeaderOverrides>> = LazyLock::new(|| {
    let source = runtime().and_then(|c| c.profile_source())?;
    let source = source.trim();
    if source.is_empty() {
        return None;
    }
    let raw = {
        let path = std::path::Path::new(source);
        if path.is_file() {
            std::fs::read_to_string(path).ok()?
        } else {
            source.to_owned()
        }
    };
    coherence::headers_from_profile(&raw).ok().flatten()
});

pub fn header_overrides() -> Option<&'static HeaderOverrides> {
    HEADER_OVERRIDES.as_ref()
}
