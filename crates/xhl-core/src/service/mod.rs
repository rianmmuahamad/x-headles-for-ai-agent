//! Service layer: aturan domain di atas driver.
//!
//! Semua validasi *sebelum* menyentuh jaringan. Service tidak tahu apa itu HTTP,
//! GraphQL, atau cookie — hanya trait driver.
//!
//! Modul dipecah per area agar tiap berkas tetap terfokus; re-export menjaga
//! `use xhl_core::service::PostService` tetap valid.

pub mod analytics;
pub mod draft;
pub mod post;
pub mod research;
pub mod scheduler;

pub use analytics::{AnalyticsService, MetricsDelta, MetricsSnapshot};
pub use draft::{Draft, DraftService};
pub use post::{validate_post, PostService, MAX_IMAGES};
pub use research::ResearchService;
pub use scheduler::{JobPayload, Scheduler};

use crate::domain::{MediaInput, Post};
use crate::driver::MAX_TEXT_LEN;
use crate::error::XhlError;

/// Panjang teks tertimbang ala X: setiap URL dihitung 23 karakter (t.co),
/// bukan panjang literalnya.
pub fn weighted_len(text: &str) -> usize {
    let mut total = 0usize;
    let bytes = text.as_bytes();
    let mut i = 0usize;

    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with("http://") || rest.starts_with("https://") {
            // URL berakhir pada whitespace pertama.
            let url_len = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
            total += 23;
            i += url_len;
            continue;
        }
        // Majukan satu karakter UTF-8 penuh.
        let step = utf8_step(bytes[i]);
        total += 1;
        i += step;
    }

    total
}

fn utf8_step(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte >> 5 == 0b110 {
        2
    } else if byte >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// Hitung gambaran media untuk `dedup_key` dan log — tidak membaca berkas.
pub fn media_fingerprint(post: &Post) -> Vec<String> {
    post.media.iter().map(MediaInput::description).collect()
}

/// Batas teks yang berlaku, diekspos agar adapter dapat menampilkannya.
pub const TEXT_LIMIT: usize = MAX_TEXT_LEN;

/// Validasi bersama yang dipakai semua jalur tulis.
pub fn ensure_text_within_limit(text: &str) -> Result<usize, XhlError> {
    let len = weighted_len(text);
    if len > MAX_TEXT_LEN {
        return Err(XhlError::Invalid(format!(
            "teks {len} karakter tertimbang melebihi batas {MAX_TEXT_LEN} \
             (URL dihitung 23 karakter)"
        )));
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_dihitung_23_karakter() {
        let t = "lihat https://contoh.example/sangat/panjang/sekali/ya/benar/sekali";
        assert_eq!(weighted_len(t), 6 + 23);
        assert_eq!(
            weighted_len("dua https://a.example dan https://b.example"),
            4 + 23 + 5 + 23
        );
    }

    #[test]
    fn hitung_karakter_utf8_bukan_byte() {
        assert_eq!(weighted_len("halo dunia"), 10);
        assert_eq!(weighted_len("日本語"), 3);
    }

    #[test]
    fn batas_teks_ditegakkan() {
        assert!(ensure_text_within_limit(&"a".repeat(MAX_TEXT_LEN)).is_ok());
        let err = ensure_text_within_limit(&"a".repeat(MAX_TEXT_LEN + 1))
            .unwrap_err()
            .to_string();
        assert!(err.contains("melebihi batas"), "{err}");
    }
}
