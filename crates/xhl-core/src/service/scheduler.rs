//! Scheduler: antrian job lokal dengan klaim atomik, retry, dan dead-letter.
//!
//! Job disimpan di tabel `jobs` sebagai JSON. Semua transisi state melalui `Store`
//! agar aman bila kelak ada lebih dari satu worker.

use std::time::Duration;

use time::OffsetDateTime;
use tokio::sync::watch;
use uuid::Uuid;

use crate::domain::Post;
use crate::driver::XWriter;
use crate::error::XhlError;
use crate::service::post::PostService;
use crate::store::{JobRow, StoreHandle};

/// Job yang macet di `Running` lebih lama dari ini dianggap milik proses yang mati.
const STALE_RUNNING: Duration = Duration::from_secs(300);

/// Maksimum percobaan sebelum job masuk dead-letter.
const MAX_ATTEMPTS: u32 = 3;

/// Isi job. Disimpan sebagai JSON di kolom `payload`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobPayload {
    Post { post: Post },
}

impl JobPayload {
    pub fn to_json(&self) -> Result<String, XhlError> {
        serde_json::to_string(self).map_err(|e| XhlError::Internal(format!("serialisasi job: {e}")))
    }

    pub fn from_json(raw: &str) -> Result<Self, XhlError> {
        serde_json::from_str(raw)
            .map_err(|e| XhlError::Invalid(format!("payload job tidak dapat dibaca: {e}")))
    }
}

pub struct Scheduler<D> {
    post: PostService<D>,
    store: StoreHandle,
    account: String,
}

impl<D: XWriter> Scheduler<D> {
    pub fn new(post: PostService<D>, store: StoreHandle, account: impl Into<String>) -> Self {
        Self {
            post,
            store,
            account: account.into(),
        }
    }

    pub async fn schedule_post(
        &self,
        at: OffsetDateTime,
        post: Post,
        dedup_key: Option<String>,
    ) -> Result<Uuid, XhlError> {
        if at < OffsetDateTime::now_utc() {
            return Err(XhlError::Invalid(
                "waktu jadwal sudah lewat; gunakan `xhl post` untuk posting langsung".into(),
            ));
        }
        let payload = JobPayload::Post { post }.to_json()?;
        let store = self.store.clone();
        let account = self.account.clone();
        let id = store
            .run(move |s| s.enqueue_job(&account, at, &payload, dedup_key.as_deref()))
            .await?;
        Uuid::parse_str(&id).map_err(|e| XhlError::Internal(format!("id job tidak valid: {e}")))
    }

    pub async fn list(&self, limit: usize) -> Result<Vec<JobRow>, XhlError> {
        let store = self.store.clone();
        let account = self.account.clone();
        store.run(move |s| s.list_jobs(&account, limit)).await
    }

    pub async fn cancel(&self, id: &Uuid) -> Result<bool, XhlError> {
        let store = self.store.clone();
        let key = id.to_string();
        store.run(move |s| s.cancel_job(&key)).await
    }

    /// Satu putaran: pulihkan job macet, ambil yang jatuh tempo, klaim, eksekusi.
    ///
    /// Mengembalikan jumlah job yang diproses. Job yang gagal dan masih boleh
    /// dicoba dijadwalkan ulang, bukan langsung dianggap selesai.
    pub async fn tick(&self) -> Result<usize, XhlError> {
        let now = OffsetDateTime::now_utc();

        // Job `Running` dari proses yang mati tidak boleh menghambat selamanya.
        let store = self.store.clone();
        let threshold = now - STALE_RUNNING;
        let recovered = store.run(move |s| s.reset_stale_running(threshold)).await?;
        if recovered > 0 {
            tracing::warn!(count = recovered, "job macet dikembalikan ke Pending");
        }

        let store = self.store.clone();
        let due = store.run(move |s| s.due_jobs(now, 50)).await?;

        let mut processed = 0usize;
        for job in due {
            let claimed = {
                let store = self.store.clone();
                let id = job.id.clone();
                store.run(move |s| s.claim_job(&id)).await?
            };
            if !claimed {
                // Sudah diklaim pihak lain.
                continue;
            }

            self.execute(&job).await?;
            processed += 1;
        }

        Ok(processed)
    }

    /// Jalankan satu job yang sudah diklaim.
    async fn execute(&self, job: &JobRow) -> Result<(), XhlError> {
        let payload = JobPayload::from_json(&job.payload)?;

        let result = match payload {
            JobPayload::Post { post } => self.post.post(post).await.map(|p| p.id.0),
        };

        match result {
            Ok(tweet_id) => {
                let store = self.store.clone();
                let id = job.id.clone();
                let marker = tweet_id.clone();
                store.run(move |s| s.finish_job(&id, Some(&marker))).await?;
                tracing::info!(job = %job.id, tweet = %tweet_id, "job selesai");
            }
            Err(e) => self.handle_failure(job, e).await?,
        }

        Ok(())
    }

    /// Kebijakan kegagalan: rate limit dijadwalkan ulang, error sementara diretry
    /// dengan backoff, sisanya masuk dead-letter.
    async fn handle_failure(&self, job: &JobRow, error: XhlError) -> Result<(), XhlError> {
        let store = self.store.clone();

        if let XhlError::RateLimited { retry_after } = &error {
            // Bukan kesalahan job: tunggu sampai jendela rate limit lewat.
            let next = OffsetDateTime::now_utc() + *retry_after;
            let id = job.id.clone();
            let attempts = job.attempts;
            store
                .run(move |s| s.reschedule_job(&id, next, attempts))
                .await?;
            tracing::warn!(
                job = %job.id,
                wait_secs = retry_after.as_secs(),
                "rate limit: job dijadwalkan ulang"
            );
            return Ok(());
        }

        let attempt_now = job.attempts + 1;

        if error.is_retryable() && attempt_now < MAX_ATTEMPTS {
            let delay = backoff(attempt_now);
            let next = OffsetDateTime::now_utc() + delay;
            let id = job.id.clone();
            store
                .run(move |s| s.reschedule_job(&id, next, attempt_now))
                .await?;
            tracing::warn!(
                job = %job.id,
                attempt = attempt_now,
                delay_secs = delay.as_secs(),
                error = %error,
                "job gagal sementara, dicoba ulang"
            );
            return Ok(());
        }

        let id = job.id.clone();
        let reason = error.to_string();
        let reason_for_store = reason.clone();
        store
            .run(move |s| s.fail_job(&id, &reason_for_store, attempt_now))
            .await?;
        tracing::error!(job = %job.id, error = %reason, "job masuk dead-letter");
        Ok(())
    }

    /// Loop sampai `shutdown` bernilai true.
    pub async fn run(
        &self,
        interval: Duration,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), XhlError> {
        let mut ticker = tokio::time::interval(interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(e) = self.tick().await {
                        // Kegagalan satu putaran tidak boleh mematikan daemon.
                        tracing::error!(error = %e, "putaran scheduler gagal");
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        tracing::info!("scheduler berhenti atas permintaan");
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Backoff eksponensial `2^n * 30s` dengan jitter, dibatasi 15 menit.
fn backoff(attempt: u32) -> Duration {
    let base = 30u64.saturating_mul(1u64 << attempt.min(5));
    let secs = base.min(900);
    let jitter = rand::random::<u64>() % 10;
    Duration::from_secs(secs + jitter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::domain::{
        Metrics, Page, Posted, SearchQuery, TimelineKind, Tweet, TweetId, UserProfile,
    };
    use crate::driver::{XReader, XWriter};
    use crate::session::SessionStatus;

    /// Driver yang selalu gagal menulis — untuk menguji jalur retry & dead-letter.
    struct FailingWriter {
        calls: AtomicU32,
        error: fn() -> XhlError,
    }

    impl FailingWriter {
        fn new(error: fn() -> XhlError) -> Self {
            Self {
                calls: AtomicU32::new(0),
                error,
            }
        }
    }

    #[async_trait]
    impl XReader for FailingWriter {
        async fn health(&self) -> Result<SessionStatus, XhlError> {
            Ok(SessionStatus::Ok {
                screen_name: "failing".into(),
                user_id: None,
            })
        }
        async fn user_by_handle(&self, h: &crate::domain::Handle) -> Result<UserProfile, XhlError> {
            Ok(UserProfile {
                id: "0".into(),
                handle: h.clone(),
                display_name: "Failing".into(),
                verified: false,
                bio: None,
                followers: None,
            })
        }
        async fn search(&self, _q: &SearchQuery) -> Result<Vec<Tweet>, XhlError> {
            Ok(Vec::new())
        }
        async fn timeline(
            &self,
            _k: TimelineKind,
            _h: Option<&crate::domain::Handle>,
            _c: Option<String>,
            _l: usize,
        ) -> Result<Page<Tweet>, XhlError> {
            Ok(Page {
                items: Vec::new(),
                next_cursor: None,
            })
        }
        async fn thread(&self, _id: &TweetId) -> Result<Vec<Tweet>, XhlError> {
            Ok(Vec::new())
        }
        async fn tweet_metrics(&self, _id: &TweetId) -> Result<Metrics, XhlError> {
            Ok(Metrics::default())
        }
    }

    #[async_trait]
    impl XWriter for FailingWriter {
        async fn upload_media(&self, _p: &Path, _a: Option<&str>) -> Result<String, XhlError> {
            Err((self.error)())
        }
        async fn post(&self, _p: &Post) -> Result<Posted, XhlError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Err((self.error)())
        }
    }

    fn past() -> OffsetDateTime {
        OffsetDateTime::now_utc() - Duration::from_secs(60)
    }

    async fn enqueue(store: &StoreHandle, at: OffsetDateTime) -> String {
        let payload = JobPayload::Post {
            post: Post::text("terjadwal"),
        }
        .to_json()
        .unwrap();
        let s = store.clone();
        s.run(move |st| st.enqueue_job("default", at, &payload, None))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn tick_mengeksekusi_job_jatuh_tempo() {
        let store = StoreHandle::in_memory().unwrap();
        enqueue(&store, past()).await;

        let post = PostService::new(crate::driver::FakeDriver::with_session("contoh"));
        let sched = Scheduler::new(post, store.clone(), "default");

        assert_eq!(sched.tick().await.unwrap(), 1);

        let jobs = sched.list(10).await.unwrap();
        assert_eq!(jobs[0].state, "Done");
    }

    #[tokio::test]
    async fn job_belum_jatuh_tempo_tidak_dieksekusi() {
        let store = StoreHandle::in_memory().unwrap();
        enqueue(
            &store,
            OffsetDateTime::now_utc() + Duration::from_secs(3600),
        )
        .await;

        let post = PostService::new(crate::driver::FakeDriver::with_session("contoh"));
        let sched = Scheduler::new(post, store.clone(), "default");

        assert_eq!(sched.tick().await.unwrap(), 0);
        assert_eq!(sched.list(10).await.unwrap()[0].state, "Pending");
    }

    #[tokio::test]
    async fn rate_limit_dijadwalkan_ulang_bukan_gagal() {
        let store = StoreHandle::in_memory().unwrap();
        enqueue(&store, past()).await;

        let post = PostService::new(FailingWriter::new(|| XhlError::RateLimited {
            retry_after: Duration::from_secs(120),
        }));
        let sched = Scheduler::new(post, store.clone(), "default");
        sched.tick().await.unwrap();

        let job = &sched.list(10).await.unwrap()[0];
        assert_eq!(
            job.state, "Pending",
            "rate limit tidak mengonsumsi percobaan"
        );
        assert_eq!(job.attempts, 0);
    }

    #[tokio::test]
    async fn error_retryable_diulang_lalu_dead_letter() {
        let store = StoreHandle::in_memory().unwrap();
        let id = enqueue(&store, past()).await;

        let post = PostService::new(FailingWriter::new(|| {
            XhlError::Network("koneksi putus".into())
        }));
        let sched = Scheduler::new(post, store.clone(), "default");

        // Tiap putaran: job dijadwalkan ulang dengan backoff. Kita majukan jadwalnya
        // agar langsung jatuh tempo pada putaran berikutnya, sampai dead-letter.
        let mut saw_pending = false;
        for _ in 0..MAX_ATTEMPTS + 1 {
            sched.tick().await.unwrap();
            let jobs = sched.list(10).await.unwrap();
            let job = &jobs[0];
            if job.state == "Failed" {
                break;
            }
            assert_eq!(job.state, "Pending", "harus diretry, bukan gagal langsung");
            saw_pending = true;

            let key = id.clone();
            let attempts = job.attempts;
            let s = store.clone();
            s.run(move |st| {
                st.reschedule_job(
                    &key,
                    OffsetDateTime::now_utc() - Duration::from_secs(1),
                    attempts,
                )
            })
            .await
            .unwrap();
        }

        assert!(saw_pending, "minimal satu percobaan ulang harus terjadi");
        let jobs = sched.list(10).await.unwrap();
        assert_eq!(
            jobs[0].state, "Failed",
            "setelah maksimum percobaan harus dead-letter"
        );
        assert_eq!(jobs[0].attempts, MAX_ATTEMPTS);
    }

    #[tokio::test]
    async fn error_butuh_manusia_langsung_dead_letter() {
        let store = StoreHandle::in_memory().unwrap();
        enqueue(&store, past()).await;

        let post = PostService::new(FailingWriter::new(|| {
            XhlError::AuthExpired("cookie kedaluwarsa".into())
        }));
        let sched = Scheduler::new(post, store.clone(), "default");
        sched.tick().await.unwrap();

        let jobs = sched.list(10).await.unwrap();
        assert_eq!(
            jobs[0].state, "Failed",
            "sesi kedaluwarsa tidak boleh diretry"
        );
        assert_eq!(jobs[0].attempts, 1);
    }

    #[tokio::test]
    async fn job_macet_dikembalikan_ke_pending() {
        let store = StoreHandle::in_memory().unwrap();
        let id = enqueue(&store, past()).await;
        {
            let s = store.clone();
            let key = id.clone();
            s.run(move |st| st.claim_job(&key)).await.unwrap();
        }

        // Job yang baru diklaim TIDAK boleh dianggap macet (proses masih hidup).
        let s = store.clone();
        let cutoff_recent = OffsetDateTime::now_utc() - STALE_RUNNING;
        assert_eq!(
            s.run(move |st| st.reset_stale_running(cutoff_recent))
                .await
                .unwrap(),
            0,
            "job yang baru diklaim bukan job macet"
        );
        assert_eq!(
            s.run(move |st| Ok(st.list_jobs("default", 10)?.remove(0).state))
                .await
                .unwrap(),
            "Running"
        );

        // Dengan batas waktu yang sudah lewat, job dianggap macet dan dikembalikan.
        let s = store.clone();
        let cutoff = OffsetDateTime::now_utc() + Duration::from_secs(60);
        assert_eq!(
            s.run(move |st| st.reset_stale_running(cutoff))
                .await
                .unwrap(),
            1
        );

        let state = s
            .run(move |st| Ok(st.list_jobs("default", 10)?.remove(0).state))
            .await
            .unwrap();
        assert_eq!(state, "Pending", "job macet harus dikembalikan ke antrian");
    }

    #[tokio::test]
    async fn tick_mengeksekusi_job_yang_baru_dipulihkan() {
        let store = StoreHandle::in_memory().unwrap();

        // Klaim lalu tandai macet secara eksplisit.
        {
            let key = enqueue(&store, past()).await;
            let s = store.clone();
            s.run(move |st| st.claim_job(&key)).await.unwrap();
            let s2 = store.clone();
            let cutoff = OffsetDateTime::now_utc() + Duration::from_secs(60);
            s2.run(move |st| st.reset_stale_running(cutoff))
                .await
                .unwrap();
        }

        let post = PostService::new(crate::driver::FakeDriver::with_session("contoh"));
        let sched = Scheduler::new(post, store.clone(), "default");
        assert_eq!(sched.tick().await.unwrap(), 1);

        let done = sched
            .list(10)
            .await
            .unwrap()
            .into_iter()
            .filter(|j| j.state == "Done")
            .count();
        assert_eq!(done, 1);
    }

    #[tokio::test]
    async fn cancel_menghentikan_job() {
        let store = StoreHandle::in_memory().unwrap();
        let id = enqueue(&store, OffsetDateTime::now_utc() + Duration::from_secs(600)).await;
        let uuid = Uuid::parse_str(&id).unwrap();

        let post = PostService::new(crate::driver::FakeDriver::with_session("contoh"));
        let sched = Scheduler::new(post, store.clone(), "default");

        assert!(sched.cancel(&uuid).await.unwrap());
        assert_eq!(sched.list(10).await.unwrap()[0].state, "Cancelled");
        assert_eq!(
            sched.tick().await.unwrap(),
            0,
            "job dibatalkan tidak dieksekusi"
        );
    }

    #[tokio::test]
    async fn jadwal_masa_lalu_ditolak() {
        let store = StoreHandle::in_memory().unwrap();
        let post = PostService::new(crate::driver::FakeDriver::with_session("contoh"));
        let sched = Scheduler::new(post, store, "default");
        assert!(sched
            .schedule_post(past(), Post::text("x"), None)
            .await
            .is_err());
        assert!(sched
            .schedule_post(
                OffsetDateTime::now_utc() + Duration::from_secs(60),
                Post::text("x"),
                None
            )
            .await
            .is_ok());
    }

    #[test]
    fn payload_bolak_balik_json() {
        let p = JobPayload::Post {
            post: Post::text("halo"),
        };
        let raw = p.to_json().unwrap();
        assert_eq!(JobPayload::from_json(&raw).unwrap(), p);
        assert!(JobPayload::from_json("{bukan json}").is_err());
    }
}
