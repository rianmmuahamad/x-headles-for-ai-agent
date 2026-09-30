//! Definisi server MCP: pemegang konfigurasi + driver yang dibangun malas (lazy).
//!
//! Server harus siap dalam ratusan milidetik; bootstrap anti-bot dan koneksi
//! database baru terjadi saat tool pertama dipanggil.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use tokio::sync::OnceCell;

use xhl_core::config::Config;
use xhl_core::driver::{BuildContext, DriverKind, XDriver};
use xhl_core::error::XhlError;
use xhl_core::store::StoreHandle;

use crate::tools::*;

/// Server MCP xhl. Satu instance = satu akun aktif.
pub struct XhlServer {
    config: Config,
    /// Store dibuka satu kali, dipakai ulang oleh semua service.
    store: OnceCell<StoreHandle>,
    /// Driver dibangun satu kali saat tool pertama benar-benar membutuhkannya.
    driver: OnceCell<Arc<dyn XDriver>>,
    /// Router tool; dibaca oleh kode yang dihasilkan `#[tool_handler]`, bukan
    /// oleh kode di berkas ini.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl XhlServer {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            store: OnceCell::new(),
            driver: OnceCell::new(),
            tool_router: Self::tool_router(),
        }
    }

    /// Buka (atau ambil) store. Menyimpan kegagalan agar tidak diulang terus-menerus.
    pub(crate) async fn store(&self) -> Result<StoreHandle, XhlError> {
        if let Some(s) = self.store.get() {
            return Ok(s.clone());
        }
        self.config.ensure_data_dir()?;
        let handle = StoreHandle::open(&self.config.db_path())?;
        let _ = self.store.set(handle);
        self.store
            .get()
            .cloned()
            .ok_or_else(|| XhlError::Internal("store gagal diinisialisasi".into()))
    }

    /// Bangun driver sesuai konfigurasi. Cookie dibaca dari store.
    pub(crate) async fn driver(&self) -> Result<Arc<dyn XDriver>, XhlError> {
        if let Some(d) = self.driver.get() {
            return Ok(Arc::clone(d));
        }

        let store = self.store().await?;
        let account = self.config.account.clone();
        let cookies = {
            let st = store.clone();
            st.run(move |s| s.cookies(&account)).await?
        };

        let ctx = BuildContext {
            cookies,
            profile: self.config.http_profile,
            account: self.config.account.clone(),
            store: Some(store),
            no_wait: self.config.no_wait,
        };
        let built = DriverKind::GraphQl.build(ctx)?;

        // `Box<dyn XDriver>` → `Arc<dyn XDriver>`: driver dipakai bersama oleh semua tool.
        let arc: Arc<dyn XDriver> = Arc::from(built);
        let _ = self.driver.set(Arc::clone(&arc));
        self.driver
            .get()
            .map(Arc::clone)
            .ok_or_else(|| XhlError::Internal("driver gagal diinisialisasi".into()))
    }

    pub(crate) fn account(&self) -> &str {
        &self.config.account
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }
}

/// Pemetaan hasil tool ke bentuk yang terlihat agent.
///
/// Kegagalan operasional X (validasi, sesi kedaluwarsa, rate limit, 403) BUKAN
/// kegagalan protokol: hasilnya dikirim sebagai `CallToolResult` dengan
/// `isError = true`, sehingga agent melihat pesan yang actionable.
/// Hanya kegagalan protokol (parameter tidak terbaca) yang memakai `ErrorData`.
///
/// Hanya `agent_safe_message()` yang keluar — pesan `Debug` mentah bisa memuat
/// detail internal dan tidak pernah dipakai.
pub(crate) fn tool_result(r: Result<String, XhlError>) -> Result<CallToolResult, ErrorData> {
    match r {
        Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)])),
        Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(
            e.agent_safe_message(),
        )])),
    }
}

#[tool_router]
impl XhlServer {
    #[tool(
        description = "Periksa status sesi X yang aktif. Mengembalikan nama akun bila sesi sehat."
    )]
    async fn xhl_session_status(&self) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_session_status().await)
    }

    #[tool(
        description = "Cari tweet. Mendukung operator X: from:, since:, -filter:replies. Konsumsi: 1 read."
    )]
    async fn xhl_search(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_search(p).await)
    }

    #[tool(
        description = "Ambil timeline (home / home_latest / user). `handle` wajib untuk kind=user. Konsumsi: 1 read."
    )]
    async fn xhl_timeline(
        &self,
        Parameters(p): Parameters<TimelineParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_timeline(p).await)
    }

    #[tool(description = "Baca isi thread dari sebuah tweet. Konsumsi: 1 read.")]
    async fn xhl_thread_read(
        &self,
        Parameters(p): Parameters<TweetIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_thread_read(p).await)
    }

    #[tool(description = "Cari profil pengguna berdasarkan handle. Konsumsi: 1 read.")]
    async fn xhl_user_lookup(
        &self,
        Parameters(p): Parameters<HandleParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_user_lookup(p).await)
    }

    #[tool(
        description = "Posting tweet teks. Konsumsi: 1 write. Posting identik pada akun yang sama tidak dikirim dua kali."
    )]
    async fn xhl_post(
        &self,
        Parameters(p): Parameters<PostParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_post(p).await)
    }

    #[tool(
        description = "Posting tweet dengan lampiran media dari path lokal. Konsumsi: 1 write + 1 media upload per berkas."
    )]
    async fn xhl_post_with_media(
        &self,
        Parameters(p): Parameters<PostMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_post_with_media(p).await)
    }

    #[tool(
        description = "Posting rangkaian thread; setiap bagian otomatis menjadi balasan bagian sebelumnya. Konsumsi: 1 write per bagian."
    )]
    async fn xhl_thread_post(
        &self,
        Parameters(p): Parameters<ThreadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_thread_post(p).await)
    }

    #[tool(
        description = "Ambil metrics sebuah tweet dan simpan snapshot lokal agar perubahan bisa dihitung. Konsumsi: 1 read."
    )]
    async fn xhl_metrics(
        &self,
        Parameters(p): Parameters<TweetIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_metrics(p).await)
    }

    #[tool(
        description = "Riwayat snapshot metrics lokal untuk sebuah tweet. Tidak menyentuh jaringan."
    )]
    async fn xhl_metrics_history(
        &self,
        Parameters(p): Parameters<MetricsHistoryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_metrics_history(p).await)
    }

    #[tool(description = "Simpan draft teks. Draft tidak pernah otomatis diposting.")]
    async fn xhl_draft_create(
        &self,
        Parameters(p): Parameters<DraftCreateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_draft_create(p).await)
    }

    #[tool(
        description = "Minta provider LLM membuat draft dari prompt. Hasilnya draft, BUKAN tweet terkirim."
    )]
    async fn xhl_draft_generate(
        &self,
        Parameters(p): Parameters<DraftGenerateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_draft_generate(p).await)
    }

    #[tool(description = "Daftar draft tersimpan.")]
    async fn xhl_draft_list(
        &self,
        Parameters(p): Parameters<LimitParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_draft_list(p).await)
    }

    #[tool(
        description = "Posting draft tersimpan. Ini satu-satunya jalur draft menjadi tweet. Konsumsi: 1 write."
    )]
    async fn xhl_post_from_draft(
        &self,
        Parameters(p): Parameters<DraftIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_post_from_draft(p).await)
    }

    #[tool(
        description = "Jadwalkan posting pada waktu tertentu (RFC3339). Job dieksekusi oleh `xhl run`."
    )]
    async fn xhl_schedule_post(
        &self,
        Parameters(p): Parameters<ScheduleParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_schedule_post(p).await)
    }

    #[tool(description = "Daftar job terjadwal.")]
    async fn xhl_schedule_list(
        &self,
        Parameters(p): Parameters<LimitParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_schedule_list(p).await)
    }

    #[tool(description = "Batalkan job terjadwal. Wajib menyetel confirm=true.")]
    async fn xhl_schedule_cancel(
        &self,
        Parameters(p): Parameters<ScheduleCancelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_schedule_cancel(p).await)
    }

    #[tool(
        description = "Status anti-bot: kesiapan signer transaction-id, query ID dikenal, dan operasi inti yang belum ter-resolve."
    )]
    async fn xhl_queries_status(&self) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_queries_status().await)
    }

    #[tool(
        description = "Posting thread dari satu berkas: bagian dipisah baris kosong ganda (atau `separator`). Konsumsi: 1 write per bagian."
    )]
    async fn xhl_thread_post_from_file(
        &self,
        Parameters(p): Parameters<ThreadFileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_thread_post_from_file(p).await)
    }

    #[tool(
        description = "Topik yang sedang ramai di X (trending/news/sport/entertainment). Konsumsi: 1 read."
    )]
    async fn xhl_trends(
        &self,
        Parameters(p): Parameters<TrendsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_trends(p).await)
    }

    #[tool(
        description = "Cari tweet dengan ranking terpilih: `top` untuk yang paling rame/relevan, `latest` untuk terbaru. Hasil menyertakan engagement. Konsumsi: 1 read."
    )]
    async fn xhl_search_ranked(
        &self,
        Parameters(p): Parameters<SearchRankedParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_search_ranked(p).await)
    }

    #[tool(
        description = "Info inti C++ native: versi, status config, profil TLS aktif, dan (opsional) nama kunci config. Nilai config tidak pernah dikembalikan."
    )]
    async fn xhl_native_info(
        &self,
        Parameters(p): Parameters<NativeInfoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_native_info(p).await)
    }

    #[tool(
        description = "Periksa koherensi identitas profil penyamaran (aturan dari camoufox): kombinasi GPU/core/layar/dpr yang mustahil dideteksi. Mengembalikan daftar pelanggaran, bukan nilai profil."
    )]
    async fn xhl_profile_check(
        &self,
        Parameters(p): Parameters<ProfileCheckParams>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_result(self.tool_profile_check(p).await)
    }
}

#[tool_handler]
impl ServerHandler for XhlServer {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::default()
    }
}
