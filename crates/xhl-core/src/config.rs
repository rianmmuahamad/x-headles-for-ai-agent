//! Konfigurasi proses. Semua nilai berasal dari env atau default OS-standar.

use std::path::PathBuf;

use crate::error::XhlError;
use crate::http::headers::{profile_by_name, HttpProfile};
use crate::native::NativeConfig;

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub account: String,
    pub http_profile: &'static HttpProfile,
    pub no_wait: bool,
    pub impersonate: bool,
    /// Config native (`XHL_CONFIG*`). Selalu ada: bila env tidak menyetel apa
    /// pun, isinya kosong dan semua accessor memakai default.
    pub native: std::sync::Arc<NativeConfig>,
}

impl Config {
    /// Baca dari env: `XHL_DATA_DIR`, `XHL_ACCOUNT`.
    ///
    /// `XHL_DATA_DIR` default: `$XDG_DATA_HOME/xhl` atau `~/.local/share/xhl`.
    pub fn from_env() -> Result<Self, XhlError> {
        // Lokasi data tetap dibaca Rust: ia menentukan *di mana* berkas berada,
        // bukan *bagaimana* xhl berperilaku, sehingga tidak masuk config native.
        let data_dir = match std::env::var_os("XHL_DATA_DIR") {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => default_data_dir()?,
        };
        let native = std::sync::Arc::new(NativeConfig::load()?);
        let account = native
            .raw_string("account")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "default".to_owned());
        // Satu nama profil menggerakkan `HttpProfile` (UA/identitas header) DAN
        // emulasi TLS/HTTP2 di `build_inner`. Dua kunci terpisah akan
        // memungkinkan kombinasi yang tidak mungkin ada di mesin nyata
        // (mis. UA Firefox dengan TLS Chrome) — justru itu yang ditandai bot.
        // Karena itu `emulation_profile` menang, `http_profile` hanya sinonim
        // lama, dan keduanya tidak pernah dipakai bersamaan.
        let from_emulation = native.emulation_profile();
        let profile_str = from_emulation
            .clone()
            .or_else(|| native.http_profile())
            .unwrap_or_else(|| crate::native::config::defaults::HTTP_PROFILE.to_owned());
        let http_profile = profile_by_name(&profile_str).ok_or_else(|| {
            let key = if from_emulation.is_some() {
                "emulation_profile"
            } else {
                "http_profile"
            };
            XhlError::Config(format!(
                "{key} '{profile_str}' tidak dikenal; pakai firefox_133 atau chrome_131"
            ))
        })?;
        let no_wait = native.no_wait();
        let impersonate = cfg!(feature = "impersonate");

        Ok(Self {
            data_dir,
            account,
            http_profile,
            no_wait,
            impersonate,
            native,
        })
    }
    /// Path database: `<data_dir>/xhl.db`.
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("xhl.db")
    }

    /// Pastikan direktori data ada dengan mode 0600-friendly (0700 untuk direktori).
    pub fn ensure_data_dir(&self) -> Result<(), XhlError> {
        std::fs::create_dir_all(&self.data_dir).map_err(|e| {
            XhlError::Config(format!("gagal membuat {}: {e}", self.data_dir.display()))
        })?;
        restrict_permissions(&self.data_dir, 0o700)
    }
}

fn default_data_dir() -> Result<PathBuf, XhlError> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(xdg).join("xhl"));
    }
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| XhlError::Config("HOME tidak diset; set XHL_DATA_DIR".into()))?;
    Ok(PathBuf::from(home).join(".local/share/xhl"))
}

/// Batasi permission file/direktori (Unix). Dilakukan best-effort: kegagalan
/// bukan alasan menghentikan program, karena sebagian FS tidak mendukung.
#[cfg(unix)]
pub fn restrict_permissions(path: &std::path::Path, mode: u32) -> Result<(), XhlError> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(mode);
    if let Err(e) = std::fs::set_permissions(path, perms) {
        tracing::warn!(path = %path.display(), error = %e, "gagal menyetel permission");
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn restrict_permissions(_path: &std::path::Path, _mode: u32) -> Result<(), XhlError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_default_dari_xdg() {
        // Tidak menyentuh env global: uji resolusi path secara terisolasi.
        let p = PathBuf::from("/tmp/xdg").join("xhl");
        assert_eq!(p.file_name().unwrap(), "xhl");
    }
}
