//! Operasi tulis: validasi, dedup, dan delegasi ke driver.

use sha2::{Digest, Sha256};

use crate::domain::{MediaInput, Post, Posted, TweetId};
use crate::driver::XWriter;
use crate::error::XhlError;
use crate::store::StoreHandle;

/// Maksimum lampiran gambar per postingan.
pub const MAX_IMAGES: usize = 4;

/// Validasi bentuk postingan. Mengembalikan pesan actionable, bukan sekadar "invalid".
pub fn validate_post(post: &Post) -> Result<(), XhlError> {
    if post.text.trim().is_empty() && post.media.is_empty() {
        return Err(XhlError::Invalid(
            "postingan harus punya teks atau media".into(),
        ));
    }

    super::ensure_text_within_limit(&post.text)?;

    let images = post
        .media
        .iter()
        .filter(|m| matches!(m, MediaInput::Image { .. }))
        .count();
    let videos = post
        .media
        .iter()
        .filter(|m| matches!(m, MediaInput::Video { .. }))
        .count();

    if images > MAX_IMAGES {
        return Err(XhlError::Invalid(format!(
            "maksimum {MAX_IMAGES} gambar per postingan, diberi {images}"
        )));
    }
    if videos > 1 {
        return Err(XhlError::Invalid("maksimum 1 video per postingan".into()));
    }
    if videos > 0 && images > 0 {
        return Err(XhlError::Invalid(
            "tidak bisa mencampur gambar dan video dalam satu postingan".into(),
        ));
    }

    Ok(())
}

/// Kunci dedup postingan: `sha256(account|text|reply_to|media)`.
///
/// Deterministik untuk input identik, sehingga pengulangan perintah yang sama
/// tidak menghasilkan tweet kedua.
pub fn dedup_key(account: &str, post: &Post) -> String {
    let raw = format!(
        "{}|{}|{:?}|{:?}",
        account,
        post.text,
        post.reply_to,
        super::media_fingerprint(post)
    );
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Batas praktis ukuran berkas konten. Postingan sendiri dibatasi 280 karakter
/// tertimbang; batas ini hanya mencegah berkas tak sengaja (mis. media) dibaca
/// sebagai teks dan mengisi memori. Dapat diatur `XHL_CONFIG`:
/// `content_max_bytes`.
pub const MAX_CONTENT_FILE_BYTES: u64 = crate::native::config::defaults::CONTENT_MAX_BYTES;

/// Batas efektif: nilai config bila ada, selain itu [`MAX_CONTENT_FILE_BYTES`].
pub fn content_max_bytes() -> u64 {
    crate::native::config_u64("content_max_bytes", MAX_CONTENT_FILE_BYTES)
}

/// Baca isi posting dari berkas yang ditulis agent.
///
/// `path == "-"` membaca stdin, sehingga agent yang menyalurkan tulisan lewat
/// pipe tidak perlu menulis berkas sementara.
///
/// Isi dikembalikan apa adanya (tidak di-trim): `validate_post` sudah memakai
/// `trim()` untuk cek kosong dan `weighted_len` untuk batas karakter.
pub fn read_content_file(path: &std::path::Path) -> Result<String, XhlError> {
    use std::io::Read;

    if path.as_os_str() == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| XhlError::Invalid(format!("gagal membaca stdin: {e}")))?;
        return Ok(buf);
    }

    let meta = std::fs::metadata(path)
        .map_err(|_| XhlError::Invalid(format!("konten tidak ditemukan: {}", path.display())))?;
    let limit = content_max_bytes();
    if meta.len() > limit {
        return Err(XhlError::Invalid(format!(
            "berkas konten terlalu besar ({} byte, maks {limit}): {}",
            meta.len(),
            path.display()
        )));
    }

    std::fs::read_to_string(path).map_err(|e| {
        XhlError::Invalid(format!(
            "berkas konten bukan UTF-8 ({e}): {}",
            path.display()
        ))
    })
}

/// Operasi tulis. Memvalidasi lebih dulu, lalu mendelegasikan ke driver.
pub struct PostService<D> {
    driver: D,
    store: Option<StoreHandle>,
    account: String,
}

impl<D: XWriter> PostService<D> {
    pub fn new(driver: D) -> Self {
        Self {
            driver,
            store: None,
            account: "default".to_string(),
        }
    }

    /// Dengan store: postingan identik pada akun yang sama tidak dikirim dua kali.
    pub fn with_store(driver: D, store: StoreHandle, account: impl Into<String>) -> Self {
        Self {
            driver,
            store: Some(store),
            account: account.into(),
        }
    }

    pub fn driver(&self) -> &D {
        &self.driver
    }

    pub async fn post(&self, post: Post) -> Result<Posted, XhlError> {
        validate_post(&post)?;

        let Some(store) = &self.store else {
            return self.driver.post(&post).await;
        };

        let key = dedup_key(&self.account, &post);

        let already = {
            let k = key.clone();
            store.run(move |s| s.posted_tweet(&k)).await?
        };

        if let Some(existing_id) = already {
            tracing::info!(id = %existing_id, "postingan sudah pernah terkirim (dedup hit)");
            return Ok(Posted {
                url: format!("https://x.com/i/status/{existing_id}"),
                id: TweetId(existing_id),
                posted_at: time::OffsetDateTime::now_utc(),
            });
        }

        let posted = self.driver.post(&post).await?;

        let k_save = key;
        let id_save = posted.id.0.clone();
        let acc_save = self.account.clone();
        if let Err(e) = store
            .run(move |s| s.record_post(&k_save, &id_save, &acc_save))
            .await
        {
            // Posting sudah terkirim; kegagalan mencatat tidak boleh menyesatkan pemanggil.
            tracing::warn!(error = %e, "gagal mencatat dedup posting");
        }

        Ok(posted)
    }

    /// Posting rangkaian: bagian ke-N otomatis menjadi balasan bagian ke-(N-1).
    ///
    /// Dijalankan berurutan karena ID bagian sebelumnya adalah input bagian
    /// berikutnya. Tiap bagian punya dedup sendiri, sehingga bagian yang sudah
    /// terkirim tidak diulang saat seluruh thread dijalankan ulang.
    pub async fn post_thread(&self, posts: Vec<Post>) -> Result<Vec<Posted>, XhlError> {
        if posts.is_empty() {
            return Err(XhlError::Invalid("thread kosong".into()));
        }
        for (i, p) in posts.iter().enumerate() {
            validate_post(p)
                .map_err(|e| XhlError::Invalid(format!("bagian {} thread: {e}", i + 1)))?;
        }

        let mut posted: Vec<Posted> = Vec::with_capacity(posts.len());
        for mut part in posts {
            if let Some(prev) = posted.last() {
                part.reply_to = Some(prev.id.clone());
            }
            posted.push(self.post(part).await?);
        }
        Ok(posted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::FakeDriver;
    use crate::driver::MAX_TEXT_LEN;
    use crate::store::StoreHandle;

    #[test]
    fn validasi_media() {
        let img = |i: usize| MediaInput::Image {
            path: format!("/tmp/{i}.jpg").into(),
            alt: None,
        };
        let four = Post {
            text: "x".into(),
            media: (0..4).map(img).collect(),
            reply_to: None,
        };
        assert!(validate_post(&four).is_ok());

        let five = Post {
            media: (0..5).map(img).collect(),
            ..Post::text("x")
        };
        assert!(validate_post(&five).is_err());

        let campur = Post {
            media: vec![
                img(0),
                MediaInput::Video {
                    path: "/tmp/v.mp4".into(),
                    alt: None,
                },
            ],
            ..Post::text("x")
        };
        assert!(
            validate_post(&campur).is_err(),
            "gambar + video tidak boleh dicampur"
        );

        let ganda = Post {
            media: vec![
                MediaInput::Video {
                    path: "/tmp/a.mp4".into(),
                    alt: None,
                },
                MediaInput::Video {
                    path: "/tmp/b.mp4".into(),
                    alt: None,
                },
            ],
            ..Post::text("x")
        };
        assert!(validate_post(&ganda).is_err(), "maksimum satu video");
    }

    #[test]
    fn postingan_kosong_ditolak() {
        assert!(validate_post(&Post::text("   ")).is_err());
        // Media saja tanpa teks tetap sah.
        let media_only = Post {
            text: String::new(),
            media: vec![MediaInput::Image {
                path: "/tmp/a.jpg".into(),
                alt: None,
            }],
            reply_to: None,
        };
        assert!(validate_post(&media_only).is_ok());
    }

    #[test]
    fn batas_teks_panjang() {
        assert!(validate_post(&Post::text("a".repeat(MAX_TEXT_LEN))).is_ok());
        let err = validate_post(&Post::text("a".repeat(MAX_TEXT_LEN + 1)))
            .unwrap_err()
            .to_string();
        assert!(err.contains("melebihi batas"), "{err}");
    }

    #[test]
    fn dedup_key_stabil_dan_berbeda() {
        let a = Post::text("halo dunia");
        let b = Post::text("halo dunia");
        assert_eq!(dedup_key("default", &a), dedup_key("default", &b));
        assert_ne!(dedup_key("default", &a), dedup_key("lain", &b));
        assert_ne!(
            dedup_key("default", &a),
            dedup_key("default", &Post::text("halo duniz"))
        );
    }

    #[tokio::test]
    async fn thread_merangkai_reply_ke_bagian_sebelumnya() {
        let svc = PostService::new(FakeDriver::with_session("contoh"));
        let out = svc
            .post_thread(vec![Post::text("1/2"), Post::text("2/2")])
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_ne!(out[0].id, out[1].id);
    }

    #[tokio::test]
    async fn thread_menolak_bagian_yang_tidak_valid() {
        let svc = PostService::new(FakeDriver::with_session("contoh"));
        let err = svc
            .post_thread(vec![Post::text("ok"), Post::text("")])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("bagian 2"), "{err}");
    }

    #[tokio::test]
    async fn thread_kosong_ditolak() {
        let svc = PostService::new(FakeDriver::with_session("contoh"));
        assert!(svc.post_thread(vec![]).await.is_err());
    }

    #[test]
    fn read_content_file_menolak_yang_tidak_layak() {
        // Tidak ada
        let err = read_content_file(std::path::Path::new("/tmp/xhl-tidak-ada-xyz.txt"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("tidak ditemukan"), "{err}");

        // Terlalu besar
        let big = std::env::temp_dir().join("xhl-big-content.txt");
        std::fs::write(&big, vec![b'a'; (MAX_CONTENT_FILE_BYTES + 1) as usize]).unwrap();
        let err = read_content_file(&big).unwrap_err().to_string();
        assert!(err.contains("terlalu besar"), "{err}");
        let _ = std::fs::remove_file(&big);

        // Bukan UTF-8
        let bad = std::env::temp_dir().join("xhl-bad-content.txt");
        std::fs::write(&bad, [0xff_u8, 0xfe, 0xfd]).unwrap();
        let err = read_content_file(&bad).unwrap_err().to_string();
        assert!(err.contains("UTF-8"), "{err}");
        let _ = std::fs::remove_file(&bad);
    }

    #[test]
    fn read_content_file_membaca_isi_apa_adanya() {
        let f = std::env::temp_dir().join("xhl-content-ok.txt");
        std::fs::write(&f, "halo\n\ndunia").unwrap();
        assert_eq!(read_content_file(&f).unwrap(), "halo\n\ndunia");
        let _ = std::fs::remove_file(&f);
    }

    #[tokio::test]
    async fn dedup_menghindari_posting_ganda() {
        let store = StoreHandle::in_memory().unwrap();
        let svc = PostService::with_store(FakeDriver::with_session("contoh"), store, "default");

        let p1 = svc.post(Post::text("tweet unik")).await.unwrap();
        let p2 = svc.post(Post::text("tweet unik")).await.unwrap();
        assert_eq!(p1.id, p2.id);

        // Teks berbeda tetap terkirim sebagai tweet baru.
        let p3 = svc.post(Post::text("tweet lain")).await.unwrap();
        assert_ne!(p1.id, p3.id);
    }
}
