//! Driver layer: abstraksi transport ke X.
//!
//! Trait dipecah menjadi pembacaan dan penulisan supaya service memegang izin
//! sesempit mungkin — `ResearchService` secara tipe tidak bisa menulis.
//!
//! Implementasi: [`FakeDriver`] (offline, untuk test & pengembangan CLI).
//! `GraphQlDriver` (cookie + HTTP/GraphQL) menyusul di M3–M5.
pub mod graphql;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use time::OffsetDateTime;

use crate::domain::{
    Author, Handle, Metrics, Page, Post, Posted, SearchQuery, TimelineKind, Trend, TrendCategory,
    Tweet, TweetId, UserProfile,
};
use crate::error::XhlError;
use crate::session::SessionStatus;

/// Operasi baca: semua yang tidak mengubah state akun.
#[async_trait]
pub trait XReader: Send + Sync {
    async fn health(&self) -> Result<SessionStatus, XhlError>;

    async fn user_by_handle(&self, handle: &Handle) -> Result<UserProfile, XhlError>;
    async fn search(&self, q: &SearchQuery) -> Result<Vec<Tweet>, XhlError>;
    async fn timeline(
        &self,
        kind: TimelineKind,
        handle: Option<&Handle>,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<Page<Tweet>, XhlError>;
    async fn thread(&self, id: &TweetId) -> Result<Vec<Tweet>, XhlError>;
    async fn tweet_metrics(&self, id: &TweetId) -> Result<Metrics, XhlError>;

    /// Topik yang sedang ramai di X.
    async fn trends(
        &self,
        _category: TrendCategory,
        _limit: usize,
    ) -> Result<Vec<Trend>, XhlError> {
        Err(XhlError::NotImplemented("trends tidak didukung driver ini"))
    }

    async fn raw_query(
        &self,
        _op: &str,
        _variables: serde_json::Value,
    ) -> Result<serde_json::Value, XhlError> {
        Err(XhlError::NotImplemented(
            "raw_query tidak didukung driver ini",
        ))
    }

    async fn diagnostics(&self) -> Option<graphql::Diagnostics> {
        None
    }
}

/// Operasi tulis: mengubah state akun, karena itu paling diawasi X.
#[async_trait]
pub trait XWriter: Send + Sync {
    async fn upload_media(&self, path: &Path, alt: Option<&str>) -> Result<String, XhlError>;
    async fn post(&self, post: &Post) -> Result<Posted, XhlError>;
}

/// Batas maksimum teks. Nilai default akun non-premium; dibuat konstanta eksplisit
/// supaya validasi tidak tersebar.
pub const MAX_TEXT_LEN: usize = 280;

/// Satu driver lengkap = bisa baca dan tulis.
pub trait XDriver: XReader + XWriter {}
impl<T: XReader + XWriter> XDriver for T {}

/// Agar `Box<dyn XDriver>` tetap memenuhi `XReader`/`XWriter` — inilah bentuk yang
/// dipakai adapter saat memilih implementasi secara runtime (registry).
#[async_trait]
impl XReader for Box<dyn XDriver> {
    async fn health(&self) -> Result<SessionStatus, XhlError> {
        (**self).health().await
    }

    async fn user_by_handle(&self, handle: &Handle) -> Result<UserProfile, XhlError> {
        (**self).user_by_handle(handle).await
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<Tweet>, XhlError> {
        (**self).search(q).await
    }

    async fn timeline(
        &self,
        kind: TimelineKind,
        handle: Option<&Handle>,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<Page<Tweet>, XhlError> {
        (**self).timeline(kind, handle, cursor, limit).await
    }

    async fn thread(&self, id: &TweetId) -> Result<Vec<Tweet>, XhlError> {
        (**self).thread(id).await
    }

    async fn tweet_metrics(&self, id: &TweetId) -> Result<Metrics, XhlError> {
        (**self).tweet_metrics(id).await
    }

    async fn trends(&self, category: TrendCategory, limit: usize) -> Result<Vec<Trend>, XhlError> {
        (**self).trends(category, limit).await
    }

    async fn raw_query(
        &self,
        op: &str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, XhlError> {
        (**self).raw_query(op, variables).await
    }

    async fn diagnostics(&self) -> Option<graphql::Diagnostics> {
        (**self).diagnostics().await
    }
}

#[async_trait]
impl XWriter for Box<dyn XDriver> {
    async fn upload_media(&self, path: &Path, alt: Option<&str>) -> Result<String, XhlError> {
        (**self).upload_media(path, alt).await
    }

    async fn post(&self, post: &Post) -> Result<Posted, XhlError> {
        (**self).post(post).await
    }
}

/// `Arc<dyn XDriver>` juga dianggap driver: adapter (mis. MCP) membagi satu
/// driver antar tool, tetapi service tetap menerima nilai `D` generik.
#[async_trait]
impl XReader for std::sync::Arc<dyn XDriver> {
    async fn health(&self) -> Result<SessionStatus, XhlError> {
        (**self).health().await
    }

    async fn user_by_handle(&self, handle: &Handle) -> Result<UserProfile, XhlError> {
        (**self).user_by_handle(handle).await
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<Tweet>, XhlError> {
        (**self).search(q).await
    }

    async fn timeline(
        &self,
        kind: TimelineKind,
        handle: Option<&Handle>,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<Page<Tweet>, XhlError> {
        (**self).timeline(kind, handle, cursor, limit).await
    }

    async fn thread(&self, id: &TweetId) -> Result<Vec<Tweet>, XhlError> {
        (**self).thread(id).await
    }

    async fn tweet_metrics(&self, id: &TweetId) -> Result<Metrics, XhlError> {
        (**self).tweet_metrics(id).await
    }

    async fn trends(&self, category: TrendCategory, limit: usize) -> Result<Vec<Trend>, XhlError> {
        (**self).trends(category, limit).await
    }

    async fn raw_query(
        &self,
        op: &str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, XhlError> {
        (**self).raw_query(op, variables).await
    }

    async fn diagnostics(&self) -> Option<graphql::Diagnostics> {
        (**self).diagnostics().await
    }
}

#[async_trait]
impl XWriter for std::sync::Arc<dyn XDriver> {
    async fn upload_media(&self, path: &Path, alt: Option<&str>) -> Result<String, XhlError> {
        (**self).upload_media(path, alt).await
    }

    async fn post(&self, post: &Post) -> Result<Posted, XhlError> {
        (**self).post(post).await
    }
}

/// Konteks pembangunan driver yang membawa kredensial, profil HTTP, dan store.
pub struct BuildContext {
    pub cookies: Option<crate::session::Cookies>,
    pub profile: &'static crate::http::headers::HttpProfile,
    pub account: String,
    pub store: Option<crate::store::StoreHandle>,
    pub no_wait: bool,
}

/// Pilih implementasi driver. Nama yang belum tersedia tetap dikenali agar
/// pengguna mendapat pesan yang tepat, bukan "unknown value".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    Fake,
    GraphQl,
}

impl DriverKind {
    /// Bangun driver sesuai pilihan.
    pub fn build(self, ctx: BuildContext) -> Result<Box<dyn XDriver>, XhlError> {
        match self {
            Self::Fake => {
                let fake_has_session = ctx.cookies.is_some();
                Ok(Box::new(if fake_has_session {
                    FakeDriver::with_session("fake-session")
                } else {
                    FakeDriver::without_session()
                }))
            }
            Self::GraphQl => {
                let cookies = ctx.cookies.ok_or_else(|| {
                    XhlError::AuthExpired("cookie belum diimpor: xhl auth import".into())
                })?;
                let client = crate::http::Client::new(ctx.profile)?;
                let signer = crate::antibot::TransactionSigner::with_session(
                    client.clone(),
                    cookies.clone(),
                    ctx.profile,
                )
                // Cache state signer agar proses berikutnya tidak perlu bootstrap
                // ulang (1–2 round-trip jaringan per perintah).
                .with_cache_store(ctx.store.clone(), ctx.account.clone());
                let registry = crate::query::QueryRegistry::new(client.clone(), ctx.store.clone());
                // Discovery memerlukan cookie sesi; halaman X versi anonim tidak
                // memuat daftar script berisi operasi GraphQL.
                registry.set_discover_headers(crate::antibot::TransactionSigner::page_headers_for(
                    &cookies,
                    ctx.profile,
                ));
                if let Some(override_path) = crate::native::runtime()
                    .and_then(|c| c.query_ids_path())
                    .filter(|p| !p.trim().is_empty())
                {
                    if let Ok(content) = std::fs::read_to_string(&override_path) {
                        if let Ok(map) = serde_json::from_str::<
                            std::collections::HashMap<String, String>,
                        >(&content)
                        {
                            registry.set_override(map);
                        }
                    }
                }
                let limiter = crate::limiter::RateLimiter::new();
                let driver = graphql::GraphQlDriver::new(graphql::GraphQlConfig {
                    client,
                    registry,
                    signer,
                    limiter,
                    cookies,
                    profile: ctx.profile,
                    account: ctx.account,
                    store: ctx.store,
                    no_wait: ctx.no_wait,
                });
                Ok(Box::new(driver))
            }
        }
    }
}

/// Basis ID tweet sintetis. Dipilih agar terlihat seperti ID X (19 digit) namun
/// jelas bukan data nyata.
const FAKE_BASE_ID: u64 = 1_700_000_000_000_000_000;

/// Batas atas hasil sintetis — mencegah alokasi tak masuk akal saat pengembangan.
const FAKE_MAX_RESULTS: usize = 100;

/// Driver offline: perilaku deterministik, tanpa jaringan.
///
/// Dipakai oleh CLI pada M1–M2 dan oleh test service/MCP. Data yang dikembalikan
/// adalah sintetis yang jelas-jelas bukan data nyata.
#[derive(Debug)]
pub struct FakeDriver {
    screen_name: Option<String>,
    post_counter: AtomicU64,
}

impl FakeDriver {
    /// Driver tanpa sesi: `health()` melaporkan `Expired`.
    pub fn without_session() -> Self {
        Self {
            screen_name: None,
            post_counter: AtomicU64::new(1000),
        }
    }

    /// Driver dengan sesi palsu aktif.
    pub fn with_session(screen_name: impl Into<String>) -> Self {
        Self {
            screen_name: Some(screen_name.into()),
            post_counter: AtomicU64::new(1000),
        }
    }
}

impl Default for FakeDriver {
    fn default() -> Self {
        Self::without_session()
    }
}

#[async_trait]
impl XReader for FakeDriver {
    async fn health(&self) -> Result<SessionStatus, XhlError> {
        Ok(match &self.screen_name {
            Some(name) => SessionStatus::Ok {
                screen_name: name.clone(),
                user_id: Some("0".into()),
            },
            None => SessionStatus::Expired {
                detail: "driver fake tanpa sesi".into(),
            },
        })
    }

    async fn user_by_handle(&self, handle: &Handle) -> Result<UserProfile, XhlError> {
        Ok(UserProfile {
            id: format!("fake-{}", handle.0),
            handle: handle.clone(),
            display_name: format!("Fake {}", handle.0),
            verified: false,
            bio: Some("profil sintetis dari FakeDriver".into()),
            followers: Some(0),
        })
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<Tweet>, XhlError> {
        if q.limit == 0 {
            return Err(XhlError::Invalid("limit harus > 0".into()));
        }
        // Cap hanya untuk mencegah alokasi konyol saat pengembangan; nilainya
        // jauh di atas limit realistis sehingga tidak menyesatkan.
        let count = q.limit.min(FAKE_MAX_RESULTS);
        Ok((0..count)
            .map(|i| {
                let id = FAKE_BASE_ID + i as u64;
                Tweet {
                    id: TweetId(id.to_string()),
                    author: Author {
                        handle: Handle::parse("fake"),
                        display_name: "Fake Driver".into(),
                        verified: false,
                    },
                    text: format!("[fake] hasil {} untuk query: {}", i + 1, q.query),
                    created_at: OffsetDateTime::UNIX_EPOCH,
                    media: vec![],
                    metrics: Some(Metrics {
                        likes: Some(0),
                        ..Default::default()
                    }),
                    url: format!("https://x.com/fake/status/{id}"),
                }
            })
            .collect())
    }

    async fn timeline(
        &self,
        kind: TimelineKind,
        handle: Option<&Handle>,
        _cursor: Option<String>,
        limit: usize,
    ) -> Result<Page<Tweet>, XhlError> {
        let label = match kind {
            TimelineKind::Home => "home".to_owned(),
            TimelineKind::HomeLatest => "home-latest".to_owned(),
            TimelineKind::User => {
                format!("user:{}", handle.map(|h| h.0.clone()).unwrap_or_default())
            }
        };
        if limit == 0 {
            return Err(XhlError::Invalid("limit harus > 0".into()));
        }
        let q = SearchQuery::new(format!("[fake timeline {label}]"), limit);
        Ok(Page {
            items: self.search(&q).await?,
            next_cursor: None,
        })
    }

    async fn thread(&self, id: &TweetId) -> Result<Vec<Tweet>, XhlError> {
        let q = SearchQuery::new(format!("[fake thread {id}]"), 2);
        Ok(self.search(&q).await?)
    }

    async fn tweet_metrics(&self, _id: &TweetId) -> Result<Metrics, XhlError> {
        Ok(Metrics {
            likes: Some(0),
            reposts: Some(0),
            replies: Some(0),
            views: Some(0),
            bookmarks: Some(0),
        })
    }

    async fn trends(&self, category: TrendCategory, limit: usize) -> Result<Vec<Trend>, XhlError> {
        if limit == 0 {
            return Err(XhlError::Invalid("limit harus > 0".into()));
        }
        let count = limit.min(FAKE_MAX_RESULTS);
        Ok((0..count)
            .map(|i| Trend {
                name: format!("#{}{}", category.as_str(), i + 1),
                rank: Some((i + 1) as u32),
                context: Some(format!("[fake] {}", category.as_str())),
                meta_description: Some(format!("{} posting", (i + 1) * 100)),
                url: Some(format!(
                    "https://x.com/search?q=%23{}{}",
                    category.as_str(),
                    i + 1
                )),
            })
            .collect())
    }
}

#[async_trait]
impl XWriter for FakeDriver {
    async fn upload_media(&self, path: &Path, _alt: Option<&str>) -> Result<String, XhlError> {
        // FakeDriver tidak membaca disk: ini menjaga test tetap murni.
        Ok(format!("fake-media-{}", path.display()))
    }

    async fn post(&self, post: &Post) -> Result<Posted, XhlError> {
        if post.text.trim().is_empty() && post.media.is_empty() {
            return Err(XhlError::Invalid("postingan kosong".into()));
        }
        let n = self.post_counter.fetch_add(1, Ordering::Relaxed);
        Ok(Posted {
            id: TweetId(format!("{n}")),
            url: format!("https://x.com/fake/status/{n}"),
            posted_at: OffsetDateTime::now_utc(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_mencerminkan_ada_tidaknya_sesi() {
        assert!(!FakeDriver::without_session()
            .health()
            .await
            .unwrap()
            .is_ok());
        assert!(FakeDriver::with_session("contoh")
            .health()
            .await
            .unwrap()
            .is_ok());
    }

    #[tokio::test]
    async fn search_menghormati_limit() {
        let d = FakeDriver::with_session("contoh");
        assert_eq!(
            d.search(&SearchQuery::new("apa", 1)).await.unwrap().len(),
            1
        );
        assert_eq!(
            d.search(&SearchQuery::new("apa", 3)).await.unwrap().len(),
            3
        );
        assert!(d.search(&SearchQuery::new("apa", 0)).await.is_err());
    }

    #[tokio::test]
    async fn fake_trends_mengembalikan_peringkat_berurutan() {
        let d = FakeDriver::with_session("contoh");
        let trends = d.trends(TrendCategory::Trending, 3).await.unwrap();
        assert_eq!(trends.len(), 3);
        assert_eq!(trends[0].rank, Some(1));
        assert_eq!(trends[2].rank, Some(3));
        assert!(trends[0].name.contains("trending"));
        assert!(d.trends(TrendCategory::News, 0).await.is_err());
    }

    #[tokio::test]
    async fn post_kosong_ditolak_dan_id_unik() {
        let d = FakeDriver::with_session("contoh");
        assert!(d.post(&Post::text("   ")).await.is_err());

        let a = d.post(&Post::text("halo")).await.unwrap();
        let b = d.post(&Post::text("halo")).await.unwrap();
        assert_ne!(a.id, b.id, "ID posting harus unik");
    }
}
