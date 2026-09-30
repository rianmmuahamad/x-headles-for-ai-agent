//! Implementasi tiap tool MCP.
//!
//! Tiap fungsi hanya: memvalidasi input, memanggil service di `xhl-core`, lalu
//! merender hasil sebagai teks ringkas. Tidak ada logika bisnis di sini.

use schemars::JsonSchema;
use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use xhl_core::domain::{
    Handle, MediaInput, Post, SearchRanking, TimelineKind, TrendCategory, TweetId,
};
use xhl_core::error::XhlError;
use xhl_core::llm::{GenOpts, NullProvider, OpenAiCompatProvider};
use xhl_core::service::{
    analytics, AnalyticsService, DraftService, PostService, ResearchService, Scheduler,
};

use crate::server::XhlServer;

// ---------------------------------------------------------------- parameter ---

/// Batas atas `limit` agar satu panggilan tool tidak menarik ribuan item.
const MAX_LIMIT: usize = 200;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Query pencarian (mendukung operator X).
    pub query: String,
    /// Jumlah maksimum hasil (1-200).
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NativeInfoParams {
    /// Sertakan daftar nama kunci config yang dikenal (nilainya tidak pernah dikembalikan).
    pub known_keys: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProfileCheckParams {
    /// Path berkas JSON profil penyamaran.
    pub profile_path: String,
    /// OS yang diklaim identitas: lin (default), win, atau mac.
    pub target_os: Option<String>,
    /// Perbaiki pelanggaran yang dapat ditentukan, lalu laporkan yang tersisa.
    pub apply: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TimelineParams {
    /// Salah satu: home, home_latest, user.
    pub kind: Option<String>,
    /// Handle (tanpa @). Wajib bila kind = user.
    pub handle: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TweetIdParams {
    /// ID numerik tweet.
    pub tweet_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HandleParams {
    /// Handle tanpa @.
    pub handle: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PostParams {
    /// Isi tweet (maksimum 280 karakter tertimbang; URL dihitung 23).
    /// Kosongkan bila memakai `content_path`.
    pub text: Option<String>,
    /// Path berkas berisi isi tweet — cara agent menyerahkan tulisannya.
    /// `-` berarti stdin. Dipakai bila `text` tidak diberikan.
    pub content_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PostMediaParams {
    /// Isi tweet. Kosongkan bila memakai `content_path`.
    pub text: Option<String>,
    /// Path berkas berisi isi tweet (`-` = stdin).
    pub content_path: Option<String>,
    /// Path lokal berkas media (maks 4 gambar atau 1 video).
    pub media_paths: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ThreadParams {
    /// Isi tiap bagian thread, berurutan.
    pub parts: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ThreadFileParams {
    /// Path berkas berisi bagian thread, satu bagian per blok (`-` = stdin).
    pub content_path: String,
    /// Pemisah antar bagian. Default baris kosong ganda.
    pub separator: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TrendsParams {
    /// Kategori: trending (default), news, sport, entertainment.
    pub category: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchRankedParams {
    pub query: String,
    /// `top` = paling rame/relevan, `latest` = terbaru (default).
    pub ranking: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MetricsHistoryParams {
    pub tweet_id: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DraftCreateParams {
    pub body: String,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DraftGenerateParams {
    /// Instruksi untuk model.
    pub prompt: String,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DraftIdParams {
    pub draft_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LimitParams {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScheduleParams {
    /// Waktu eksekusi RFC3339, contoh 2026-10-01T09:00:00+07:00.
    pub at: String,
    /// Isi tweet.
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScheduleCancelParams {
    pub job_id: String,
    /// Wajib true; mencegah pembatalan tak sengaja.
    pub confirm: bool,
}

/// Ambil isi posting dari `text` atau `content_path`.
///
/// Aturan: isi hanya salah satu. Pengecualian `allow_empty_both` dipakai oleh
/// jalur media, di mana postingan boleh hanya berisi lampiran.
fn content_from(
    text: Option<String>,
    content_path: Option<String>,
    allow_empty_both: bool,
) -> Result<String, XhlError> {
    match (text, content_path) {
        (Some(_), Some(_)) => Err(XhlError::Invalid(
            "isi hanya salah satu: `text` atau `content_path`".into(),
        )),
        (Some(t), None) => Ok(t),
        (None, Some(path)) => {
            xhl_core::service::post::read_content_file(std::path::Path::new(&path))
        }
        (None, None) if allow_empty_both => Ok(String::new()),
        (None, None) => Err(XhlError::Invalid(
            "berikan `text` atau `content_path`".into(),
        )),
    }
}

/// Terjemahkan nama kategori trend. Nilai tak dikenal ditolak dengan daftar sah.
fn parse_trend_category(raw: Option<&str>) -> Result<TrendCategory, XhlError> {
    match raw.unwrap_or("trending").to_ascii_lowercase().as_str() {
        "trending" => Ok(TrendCategory::Trending),
        "news" => Ok(TrendCategory::News),
        "sport" => Ok(TrendCategory::Sport),
        "entertainment" => Ok(TrendCategory::Entertainment),
        other => Err(XhlError::Invalid(format!(
            "kategori '{other}' tidak dikenal; pakai trending, news, sport, atau entertainment"
        ))),
    }
}

/// Terjemahkan nama ranking. Nilai tak dikenal ditolak dengan daftar sah.
fn parse_ranking(raw: Option<&str>) -> Result<SearchRanking, XhlError> {
    match raw.unwrap_or("latest").to_ascii_lowercase().as_str() {
        "latest" => Ok(SearchRanking::Latest),
        "top" => Ok(SearchRanking::Top),
        other => Err(XhlError::Invalid(format!(
            "ranking '{other}' tidak dikenal; pakai top atau latest"
        ))),
    }
}

fn clamp_limit(limit: Option<usize>, default: usize) -> usize {
    limit.unwrap_or(default).clamp(1, MAX_LIMIT)
}

fn render_tweets(tweets: &[xhl_core::domain::Tweet]) -> String {
    if tweets.is_empty() {
        return "tidak ada hasil".to_owned();
    }
    tweets
        .iter()
        .map(|t| {
            format!(
                "{} (@{}) {}\n{}\n{}\n{}",
                t.id,
                t.author.handle.0,
                t.created_at.format(&Rfc3339).unwrap_or_default(),
                t.text.replace('\n', " "),
                t.metrics
                    .as_ref()
                    .map(|m| m.summary())
                    .unwrap_or_else(|| "tanpa metrik".to_owned()),
                t.url
            )
        })
        .collect::<Vec<_>>()
        .join("\n---\n")
}

// -------------------------------------------------------------------- tools ---

impl XhlServer {
    pub(crate) async fn tool_session_status(&self) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let status = d.health().await?;
        let out = match status {
            xhl_core::session::SessionStatus::Ok { screen_name, .. } => {
                format!("sesi sehat: @{screen_name} (akun '{}')", self.account())
            }
            xhl_core::session::SessionStatus::Expired { detail } => format!(
                "sesi TIDAK valid ({detail}). Minta pengguna menjalankan: xhl auth import --from cookies.txt"
            ),
        };
        Ok(out)
    }

    pub(crate) async fn tool_search(&self, p: SearchParams) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let svc = ResearchService::new(d);
        let limit = clamp_limit(p.limit, 20);
        let tweets = svc.search(&p.query, limit).await?;
        Ok(render_tweets(&tweets))
    }

    pub(crate) async fn tool_timeline(&self, p: TimelineParams) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let svc = ResearchService::new(d);
        let limit = clamp_limit(p.limit, 20);

        let kind = match p
            .kind
            .as_deref()
            .unwrap_or("home")
            .to_ascii_lowercase()
            .as_str()
        {
            "home" => TimelineKind::Home,
            "home_latest" | "latest" => TimelineKind::HomeLatest,
            "user" => TimelineKind::User,
            other => {
                return Err(XhlError::Invalid(format!(
                    "kind '{other}' tidak dikenal; pakai home, home_latest, atau user"
                )))
            }
        };

        let handle = match (kind, &p.handle) {
            (TimelineKind::User, Some(h)) => Some(Handle::parse(h)),
            (TimelineKind::User, None) => {
                return Err(XhlError::Invalid("kind=user memerlukan handle".into()))
            }
            _ => None,
        };

        let page = svc.timeline(kind, handle.as_ref(), None, limit).await?;

        let mut out = render_tweets(&page.items);
        if let Some(c) = page.next_cursor {
            out.push_str(&format!("\n(cursor berikutnya: {c})"));
        }
        Ok(out)
    }

    pub(crate) async fn tool_thread_read(&self, p: TweetIdParams) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let svc = ResearchService::new(d);
        let tweets = svc.thread(&TweetId(p.tweet_id)).await?;
        Ok(render_tweets(&tweets))
    }

    pub(crate) async fn tool_user_lookup(&self, p: HandleParams) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let svc = ResearchService::new(d);
        let u = svc.user(&Handle::parse(&p.handle)).await?;
        Ok(format!(
            "{} ({})\nid: {}\nverified: {}\nfollowers: {}\n{}",
            u.display_name,
            u.handle,
            u.id,
            u.verified,
            u.followers
                .map(|f| f.to_string())
                .unwrap_or_else(|| "-".into()),
            u.bio.unwrap_or_default()
        ))
    }

    pub(crate) async fn tool_post(&self, p: PostParams) -> Result<String, XhlError> {
        let body = content_from(p.text, p.content_path, false)?;
        let d = self.driver().await?;
        let store = self.store().await?;
        let svc = PostService::with_store(d, store, self.account());
        let posted = svc.post(Post::text(body)).await?;
        Ok(format!("terkirim: {}\nid: {}", posted.url, posted.id))
    }

    pub(crate) async fn tool_post_with_media(
        &self,
        p: PostMediaParams,
    ) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let store = self.store().await?;
        let svc = PostService::with_store(d, store, self.account());

        // Postingan boleh hanya berisi media, jadi teks kosong diizinkan di sini.
        let body = content_from(p.text, p.content_path, true)?;

        let media: Vec<MediaInput> = p
            .media_paths
            .into_iter()
            .map(MediaInput::from_path)
            .collect();
        let post = Post {
            text: body,
            media,
            reply_to: None,
        };
        let posted = svc.post(post).await?;
        Ok(format!(
            "terkirim (dengan media): {}\nid: {}",
            posted.url, posted.id
        ))
    }

    pub(crate) async fn tool_thread_post(&self, p: ThreadParams) -> Result<String, XhlError> {
        if p.parts.is_empty() {
            return Err(XhlError::Invalid("parts tidak boleh kosong".into()));
        }

        let d = self.driver().await?;
        let store = self.store().await?;
        let svc = PostService::with_store(d, store, self.account());

        let posts = p.parts.into_iter().map(Post::text).collect();
        let posted = svc.post_thread(posts).await?;
        let lines: Vec<String> = posted
            .iter()
            .enumerate()
            .map(|(i, p)| format!("{}/{}: {}", i + 1, posted.len(), p.url))
            .collect();
        Ok(format!("thread terkirim:\n{}", lines.join("\n")))
    }

    /// Thread yang ditulis agent sebagai satu berkas, satu bagian per blok.
    pub(crate) async fn tool_thread_post_from_file(
        &self,
        p: ThreadFileParams,
    ) -> Result<String, XhlError> {
        let raw =
            xhl_core::service::post::read_content_file(std::path::Path::new(&p.content_path))?;
        let separator = p.separator.unwrap_or_else(|| "\n\n".to_owned());

        let parts: Vec<Post> = raw
            .split(&separator)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| Post::text(s.to_owned()))
            .collect();

        if parts.is_empty() {
            return Err(XhlError::Invalid(
                "berkas thread tidak memuat bagian".into(),
            ));
        }

        let d = self.driver().await?;
        let store = self.store().await?;
        let svc = PostService::with_store(d, store, self.account());
        let posted = svc.post_thread(parts).await?;

        let lines: Vec<String> = posted
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{}/{}: {}", i + 1, posted.len(), t.url))
            .collect();
        Ok(format!("thread terkirim:\n{}", lines.join("\n")))
    }

    pub(crate) async fn tool_trends(&self, p: TrendsParams) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let svc = ResearchService::new(d);
        let category = parse_trend_category(p.category.as_deref())?;
        let limit = clamp_limit(p.limit, 20);

        let trends = svc.trends(category, limit).await?;
        if trends.is_empty() {
            return Ok("tidak ada trend".to_owned());
        }
        Ok(trends
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let rank = t.rank.unwrap_or((i + 1) as u32);
                let ctx = t.context.as_deref().unwrap_or("");
                let extra = t.meta_description.as_deref().unwrap_or("");
                format!("{rank}. {} {ctx} {extra}", t.name)
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    pub(crate) async fn tool_search_ranked(
        &self,
        p: SearchRankedParams,
    ) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let svc = ResearchService::new(d);
        let ranking = parse_ranking(p.ranking.as_deref())?;
        let limit = clamp_limit(p.limit, 20);

        let tweets = svc.search_ranked(&p.query, limit, ranking).await?;
        Ok(render_tweets(&tweets))
    }

    pub(crate) async fn tool_metrics(&self, p: TweetIdParams) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let store = self.store().await?;
        let svc = AnalyticsService::new(d, store);
        let id = TweetId(p.tweet_id);
        let m = svc.capture(&id).await?;
        let count = svc.snapshot_count(&id).await?;

        let f = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_else(|| "-".into());
        Ok(format!(
            "tweet {}\nlikes={} reposts={} replies={} views={} bookmarks={}\nsnapshot tersimpan: {count}",
            id.0,
            f(m.likes),
            f(m.reposts),
            f(m.replies),
            f(m.views),
            f(m.bookmarks)
        ))
    }

    pub(crate) async fn tool_metrics_history(
        &self,
        p: MetricsHistoryParams,
    ) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let store = self.store().await?;
        let svc = AnalyticsService::new(d, store);
        let limit = clamp_limit(p.limit, 50);
        let rows = svc.history(&TweetId(p.tweet_id), limit).await?;

        if rows.is_empty() {
            return Ok("belum ada snapshot untuk tweet ini".to_owned());
        }
        Ok(rows
            .iter()
            .map(analytics::snapshot_line)
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn draft_service(&self) -> Result<DraftService<XDriverArc>, XhlError> {
        let d = self.driver().await?;
        let store = self.store().await?;

        let llm: Box<dyn xhl_core::llm::LlmProvider> =
            match OpenAiCompatProvider::from_env(self.config().http_profile) {
                Ok(Some(p)) => Box::new(p),
                _ => Box::new(NullProvider),
            };

        let poster = PostService::with_store(d, store.clone(), self.account());
        Ok(DraftService::new(store, llm, self.account(), poster))
    }

    pub(crate) async fn tool_draft_create(&self, p: DraftCreateParams) -> Result<String, XhlError> {
        let svc = self.draft_service().await?;
        let d = svc.create(p.name, p.body, Vec::new()).await?;
        Ok(format!("draft tersimpan: {}\n(belum diposting)", d.id))
    }

    pub(crate) async fn tool_draft_generate(
        &self,
        p: DraftGenerateParams,
    ) -> Result<String, XhlError> {
        let svc = self.draft_service().await?;
        let d = svc.generate(&p.prompt, p.name, GenOpts::default()).await?;
        Ok(format!(
            "draft dari LLM: {}\n---\n{}\n---\nbelum diposting; gunakan xhl_post_from_draft untuk mengirim",
            d.id, d.body
        ))
    }

    pub(crate) async fn tool_draft_list(&self, p: LimitParams) -> Result<String, XhlError> {
        let svc = self.draft_service().await?;
        let rows = svc.list(clamp_limit(p.limit, 20)).await?;
        if rows.is_empty() {
            return Ok("belum ada draft".to_owned());
        }
        Ok(rows
            .iter()
            .map(xhl_core::service::draft::draft_line)
            .collect::<Vec<_>>()
            .join("\n"))
    }

    pub(crate) async fn tool_post_from_draft(&self, p: DraftIdParams) -> Result<String, XhlError> {
        let svc = self.draft_service().await?;
        let posted = svc.post(&p.draft_id).await?;
        Ok(format!("terkirim: {}\nid: {}", posted.url, posted.id))
    }

    pub(crate) async fn tool_schedule_post(&self, p: ScheduleParams) -> Result<String, XhlError> {
        let when = OffsetDateTime::parse(&p.at, &Rfc3339)
            .map_err(|e| XhlError::Invalid(format!("`at` bukan RFC3339 valid ({e})")))?;

        let d = self.driver().await?;
        let store = self.store().await?;
        let poster = PostService::with_store(d, store.clone(), self.account());
        let sched = Scheduler::new(poster, store, self.account());

        let post = Post::text(p.text);
        let key = xhl_core::service::post::dedup_key(self.account(), &post);
        let id = sched.schedule_post(when, post, Some(key)).await?;

        Ok(format!(
            "job terjadwal: {id}\nwaktu: {}\njob dieksekusi oleh daemon `xhl run`",
            p.at
        ))
    }

    pub(crate) async fn tool_schedule_list(&self, p: LimitParams) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let store = self.store().await?;
        let poster = PostService::with_store(d, store.clone(), self.account());
        let sched = Scheduler::new(poster, store, self.account());

        let rows = sched.list(clamp_limit(p.limit, 20)).await?;
        if rows.is_empty() {
            return Ok("belum ada job terjadwal".to_owned());
        }
        Ok(rows
            .iter()
            .map(|j| {
                format!(
                    "{}  {}  {}  attempts={}",
                    j.id,
                    j.run_at.format(&Rfc3339).unwrap_or_default(),
                    j.state,
                    j.attempts
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    pub(crate) async fn tool_schedule_cancel(
        &self,
        p: ScheduleCancelParams,
    ) -> Result<String, XhlError> {
        if !p.confirm {
            return Err(XhlError::Invalid("pembatalan butuh confirm=true".into()));
        }
        let uuid = Uuid::parse_str(&p.job_id)
            .map_err(|e| XhlError::Invalid(format!("job_id bukan UUID valid: {e}")))?;

        let d = self.driver().await?;
        let store = self.store().await?;
        let poster = PostService::with_store(d, store.clone(), self.account());
        let sched = Scheduler::new(poster, store, self.account());

        if !sched.cancel(&uuid).await? {
            return Err(XhlError::Invalid(
                "job tidak ditemukan atau sudah final".into(),
            ));
        }
        Ok(format!("job dibatalkan: {}", p.job_id))
    }

    pub(crate) async fn tool_queries_status(&self) -> Result<String, XhlError> {
        let d = self.driver().await?;
        let diag = d.diagnostics().await.ok_or(XhlError::NotImplemented(
            "diagnostics hanya tersedia untuk driver GraphQL",
        ))?;

        let mut out = format!(
            "signer transaction-id: {}\nquery ID dikenal: {}",
            if diag.signer_ready {
                "siap".to_owned()
            } else {
                "belum bootstrap (akan dibuat saat request pertama)".to_owned()
            },
            diag.known_operations.len()
        );
        if diag.missing_operations.is_empty() {
            out.push_str("\nsemua operasi inti tersedia");
        } else {
            out.push_str(&format!(
                "\noperasi inti belum ter-resolve (vital untuk create/read): {}",
                diag.missing_operations.join(", ")
            ));
        }
        Ok(out)
    }

    /// Info inti native. Hanya **nama** kunci config yang dikembalikan: nilai
    /// config dapat memuat path/rahasia dan tidak boleh sampai ke agent.
    pub(crate) async fn tool_native_info(&self, p: NativeInfoParams) -> Result<String, XhlError> {
        let mut out = format!(
            "native    : {}\naktif     : ya (config dibaca lapisan C++)",
            xhl_core::native::ffi::version()
        );
        let cfg = xhl_core::native::NativeConfig::load()?;
        if let Some(err) = cfg.load_error() {
            out.push_str(&format!("\nperingatan: {err}"));
        }
        out.push_str(&format!(
            "\nprofil TLS: {}",
            self.config().http_profile.name
        ));
        if p.known_keys.unwrap_or(false) {
            let keys = cfg.known_keys()?;
            out.push_str(&format!(
                "\nkunci config dikenal ({}):\n  {}",
                keys.len(),
                keys.join("\n  ")
            ));
        }
        Ok(out)
    }

    /// Periksa koherensi profil dari berkas. Hasilnya hanya daftar pelanggaran
    /// (`rule`, `detail`) — nilai profil tidak dikembalikan.
    pub(crate) async fn tool_profile_check(
        &self,
        p: ProfileCheckParams,
    ) -> Result<String, XhlError> {
        let target_os = p.target_os.as_deref().unwrap_or("lin");
        if !matches!(target_os, "lin" | "win" | "mac") {
            return Err(XhlError::Invalid(format!(
                "target_os tidak dikenal: '{target_os}'; pakai lin, win, atau mac"
            )));
        }
        let raw = std::fs::read_to_string(&p.profile_path).map_err(|e| {
            XhlError::Invalid(format!(
                "profil tidak dapat dibaca ({}): {e}",
                p.profile_path
            ))
        })?;

        if p.apply.unwrap_or(false) {
            let report = xhl_core::native::coherence::apply_coherence(&raw, target_os)?;
            let mut out = format!("target_os: {target_os}\nprofil diperbaiki.\n");
            if report.violations.is_empty() {
                out.push_str("tidak ada pelanggaran tersisa (koheren)");
            } else {
                out.push_str("pelanggaran tersisa:\n");
                for v in &report.violations {
                    out.push_str(&format!("  [{}] {}\n", v.rule, v.detail));
                }
            }
            return Ok(out);
        }

        let violations = xhl_core::native::coherence::check_profile(&raw, target_os)?;
        if violations.is_empty() {
            return Ok(format!("target_os: {target_os}\nidentitas koheren"));
        }
        let mut out = format!(
            "target_os: {target_os}\n{} pelanggaran:\n",
            violations.len()
        );
        for v in &violations {
            out.push_str(&format!("  [{}] {}\n", v.rule, v.detail));
        }
        Ok(out)
    }
}

/// Adapter kepemilikan: driver dibagi lewat `Arc`, sehingga semua tool memakai
/// satu koneksi HTTP dan satu state anti-bot.
pub(crate) type XDriverArc = std::sync::Arc<dyn xhl_core::driver::XDriver>;
