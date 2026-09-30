//! Draft: penyimpanan lokal untuk teks yang belum diposting.
//!
//! Draft dari LLM **tidak pernah** otomatis diposting; `post()` adalah aksi eksplisit.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::{MediaInput, Post, Posted};
use crate::driver::XWriter;
use crate::error::XhlError;
use crate::llm::{GenOpts, LlmProvider};
use crate::service::post::PostService;
use crate::store::{DraftRow, StoreHandle};

/// Nilai `origin` yang dikenal.
pub const ORIGIN_HUMAN: &str = "human";
pub const ORIGIN_LLM: &str = "llm";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub id: String,
    pub name: Option<String>,
    pub body: String,
    pub origin: String,
    pub media: Vec<MediaInput>,
    pub created_at: OffsetDateTime,
}

impl Draft {
    /// Draft sebagai input posting, tanpa media bila tidak bisa dibaca.
    pub fn to_post(&self) -> Post {
        Post {
            text: self.body.clone(),
            media: self.media.clone(),
            reply_to: None,
        }
    }

    pub fn is_from_llm(&self) -> bool {
        self.origin == ORIGIN_LLM
    }
}

fn parse_media(media_json: &str) -> Vec<MediaInput> {
    // Draft lama (sebelum migrasi v2) memakai default '[]'.
    serde_json::from_str(media_json).unwrap_or_default()
}

fn from_row(row: DraftRow) -> Draft {
    Draft {
        id: row.id,
        name: row.name,
        body: row.body,
        origin: row.origin,
        media: parse_media(&row.media_json),
        created_at: row.created_at,
    }
}

pub struct DraftService<D: XWriter> {
    store: StoreHandle,
    llm: Box<dyn LlmProvider>,
    account: String,
    poster: PostService<D>,
}

impl<D: XWriter> DraftService<D> {
    pub fn new(
        store: StoreHandle,
        llm: Box<dyn LlmProvider>,
        account: impl Into<String>,
        poster: PostService<D>,
    ) -> Self {
        Self {
            store,
            llm,
            account: account.into(),
            poster,
        }
    }

    /// Simpan draft yang ditulis manusia.
    pub async fn create(
        &self,
        name: Option<String>,
        body: String,
        media: Vec<MediaInput>,
    ) -> Result<Draft, XhlError> {
        self.insert(name, body, media, ORIGIN_HUMAN).await
    }

    /// Minta teks ke provider LLM lalu simpan sebagai draft (`origin = "llm"`).
    pub async fn generate(
        &self,
        prompt: &str,
        name: Option<String>,
        opts: GenOpts,
    ) -> Result<Draft, XhlError> {
        if prompt.trim().is_empty() {
            return Err(XhlError::Invalid("prompt kosong".into()));
        }
        let body = self.llm.generate(prompt, opts).await?;
        let body = body.trim().to_string();
        if body.is_empty() {
            return Err(XhlError::Internal(format!(
                "provider '{}' mengembalikan teks kosong",
                self.llm.name()
            )));
        }
        self.insert(name, body, Vec::new(), ORIGIN_LLM).await
    }

    async fn insert(
        &self,
        name: Option<String>,
        body: String,
        media: Vec<MediaInput>,
        origin: &'static str,
    ) -> Result<Draft, XhlError> {
        if body.trim().is_empty() && media.is_empty() {
            return Err(XhlError::Invalid("draft kosong".into()));
        }

        let id = Uuid::new_v4().to_string();
        let media_json = serde_json::to_string(&media)
            .map_err(|e| XhlError::Internal(format!("serialisasi media draft: {e}")))?;

        let store = self.store.clone();
        let account = self.account.clone();
        let id_save = id.clone();
        let name_save = name.clone();
        let body_save = body.clone();
        store
            .run(move |s| {
                s.insert_draft(
                    &id_save,
                    &account,
                    &body_save,
                    name_save.as_deref(),
                    origin,
                    &media_json,
                )
            })
            .await?;

        Ok(Draft {
            id,
            name,
            body,
            origin: origin.to_string(),
            media,
            created_at: OffsetDateTime::now_utc(),
        })
    }

    pub async fn list(&self, limit: usize) -> Result<Vec<Draft>, XhlError> {
        if limit == 0 {
            return Err(XhlError::Invalid("limit harus > 0".into()));
        }
        let store = self.store.clone();
        let account = self.account.clone();
        let rows = store.run(move |s| s.list_drafts(&account, limit)).await?;
        Ok(rows.into_iter().map(from_row).collect())
    }

    pub async fn get(&self, id: &str) -> Result<Option<Draft>, XhlError> {
        let store = self.store.clone();
        let key = id.to_string();
        let row = store.run(move |s| s.get_draft(&key)).await?;
        Ok(row.map(from_row))
    }

    pub async fn delete(&self, id: &str) -> Result<bool, XhlError> {
        let store = self.store.clone();
        let key = id.to_string();
        store.run(move |s| s.delete_draft(&key)).await
    }

    /// Posting draft = aksi eksplisit. Draft tidak dihapus bila posting gagal.
    pub async fn post(&self, id: &str) -> Result<Posted, XhlError> {
        let draft = self
            .get(id)
            .await?
            .ok_or_else(|| XhlError::Invalid(format!("draft '{id}' tidak ditemukan")))?;
        self.poster.post(draft.to_post()).await
    }

    /// Nama provider yang aktif (untuk ditampilkan adapter).
    pub fn llm_name(&self) -> &'static str {
        self.llm.name()
    }
}

/// Baris ringkas untuk ditampilkan adapter.
pub fn draft_line(d: &Draft) -> String {
    let preview: String = d.body.chars().take(60).collect();
    let name = d.name.as_deref().unwrap_or("-");
    format!(
        "{}  [{}] {}  {}  ({})",
        d.id,
        d.origin,
        name,
        preview.replace('\n', " "),
        crate::store::format_rfc3339(d.created_at),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::FakeDriver;
    use crate::llm::NullProvider;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Provider yang selalu mengembalikan teks tetap.
    struct StubProvider {
        text: String,
        calls: AtomicU32,
    }

    #[async_trait::async_trait]
    impl LlmProvider for StubProvider {
        async fn generate(&self, _prompt: &str, _opts: GenOpts) -> Result<String, XhlError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.text.clone())
        }
        fn name(&self) -> &'static str {
            "stub"
        }
    }

    fn service(llm: Box<dyn LlmProvider>) -> (DraftService<FakeDriver>, StoreHandle) {
        let store = StoreHandle::in_memory().unwrap();
        let poster =
            PostService::with_store(FakeDriver::with_session("contoh"), store.clone(), "default");
        (
            DraftService::new(store.clone(), llm, "default", poster),
            store,
        )
    }

    #[tokio::test]
    async fn create_menyimpan_dengan_origin_human() {
        let (svc, _) = service(Box::new(NullProvider));
        let d = svc
            .create(Some("peluncuran".into()), "teks draft".into(), Vec::new())
            .await
            .unwrap();
        assert_eq!(d.origin, "human");
        assert!(!d.is_from_llm());

        let listed = svc.list(10).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].body, "teks draft");

        let fetched = svc.get(&d.id).await.unwrap().unwrap();
        assert_eq!(fetched.name.as_deref(), Some("peluncuran"));
    }

    #[tokio::test]
    async fn null_provider_gagal_dengan_pesan_actionable() {
        let (svc, _) = service(Box::new(NullProvider));
        let err = svc
            .generate("tweet tentang Rust", None, GenOpts::default())
            .await
            .unwrap_err();
        match err {
            XhlError::Config(msg) => {
                assert!(msg.contains("OPENAI_BASE_URL"), "{msg}");
                assert!(msg.contains("OPENAI_API_KEY"), "{msg}");
            }
            other => panic!("harus Config error, dapat: {other}"),
        }
        // Tidak ada draft tersimpan saat provider gagal.
        assert!(svc.list(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn generate_dari_llm_bertanda_llm_dan_tidak_memposting() {
        let provider = Box::new(StubProvider {
            text: "  teks dari model  ".into(),
            calls: AtomicU32::new(0),
        });
        let (svc, store) = service(provider);

        let d = svc
            .generate("prompt", None, GenOpts::default())
            .await
            .unwrap();
        assert_eq!(d.origin, "llm");
        assert!(d.is_from_llm());
        assert_eq!(d.body, "teks dari model", "spasi tepi dipangkas");

        // Belum ada posting sama sekali.
        let posts = store.clone();
        let key = crate::service::post::dedup_key("default", &d.to_post());
        let posted = posts.run(move |s| s.posted_tweet(&key)).await.unwrap();
        assert!(posted.is_none(), "generate tidak boleh memposting");
    }

    #[tokio::test]
    async fn generate_menolak_prompt_kosong_dan_teks_kosong() {
        let (svc, _) = service(Box::new(StubProvider {
            text: "   ".into(),
            calls: AtomicU32::new(0),
        }));
        assert!(svc.generate("  ", None, GenOpts::default()).await.is_err());
        assert!(
            svc.generate("prompt", None, GenOpts::default())
                .await
                .is_err(),
            "teks kosong dari provider harus ditolak"
        );
    }

    #[tokio::test]
    async fn post_draft_mengirim_dan_menghormati_dedup() {
        let (svc, _) = service(Box::new(NullProvider));
        let d = svc
            .create(None, "isi draft".into(), Vec::new())
            .await
            .unwrap();

        let p1 = svc.post(&d.id).await.unwrap();
        let p2 = svc.post(&d.id).await.unwrap();
        assert_eq!(
            p1.id, p2.id,
            "posting draft dua kali tidak menghasilkan tweet kedua"
        );
    }

    #[tokio::test]
    async fn post_draft_tidak_dikenal_ditolak() {
        let (svc, _) = service(Box::new(NullProvider));
        let err = svc.post("tidak-ada").await.unwrap_err().to_string();
        assert!(err.contains("tidak ditemukan"), "{err}");
    }

    #[tokio::test]
    async fn media_draft_bolak_balik() {
        let (svc, _) = service(Box::new(NullProvider));
        let media = vec![MediaInput::Image {
            path: "/tmp/a.jpg".into(),
            alt: Some("alt".into()),
        }];
        let d = svc
            .create(None, "dengan media".into(), media.clone())
            .await
            .unwrap();

        let fetched = svc.get(&d.id).await.unwrap().unwrap();
        assert_eq!(fetched.media, media);
    }

    #[tokio::test]
    async fn delete_draft() {
        let (svc, _) = service(Box::new(NullProvider));
        let d = svc
            .create(None, "hapus aku".into(), Vec::new())
            .await
            .unwrap();
        assert!(svc.delete(&d.id).await.unwrap());
        assert!(svc.get(&d.id).await.unwrap().is_none());
        assert!(!svc.delete(&d.id).await.unwrap());
    }
}
