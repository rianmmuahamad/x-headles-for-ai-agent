//! CLI `xhl` — adapter tipis di atas `xhl-core`.
//!
//! Aturan adapter: CLI hanya memetakan argumen ke pemanggilan domain dan
//! merender hasil. Tidak ada logika bisnis di sini.

use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use xhl_core::config::Config;
use xhl_core::domain::{Handle, Post, TimelineKind, TweetId};
use xhl_core::driver::{DriverKind, XDriver};
use xhl_core::error::XhlError;
use xhl_core::llm::{GenOpts, LlmProvider, NullProvider, OpenAiCompatProvider};
use xhl_core::service::{
    analytics, draft as draft_mod, AnalyticsService, DraftService, PostService, ResearchService,
    Scheduler,
};
use xhl_core::session::{Cookies, SessionStatus};
use xhl_core::store::StoreHandle;

#[derive(Parser, Debug)]
#[command(
    name = "xhl",
    version,
    about = "X Headless — kendalikan akun X dari CLI",
    long_about = "Kontrol akun X untuk agent & manusia: posting, riset, analytics, jadwal, dan draft."
)]
struct Cli {
    /// Akun yang dipakai (default: $XHL_ACCOUNT atau "default").
    #[arg(long, global = true)]
    account: Option<String>,

    /// Pilih implementasi driver.
    #[arg(long, global = true, value_enum, default_value_t = DriverArg::Graphql)]
    driver: DriverArg,

    /// Keluarkan JSON alih-alih teks.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum DriverArg {
    /// Driver HTTP/GraphQL ke X memakai cookie yang diimpor (default).
    Graphql,
    /// Driver offline untuk pengembangan & uji (data sintetis, tanpa jaringan).
    Fake,
}

impl DriverArg {
    fn kind(self) -> DriverKind {
        match self {
            Self::Graphql => DriverKind::GraphQl,
            Self::Fake => DriverKind::Fake,
        }
    }
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Kelola kredensial sesi (cookie).
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Cari tweet.
    Search {
        /// Query pencarian (mendukung operator X: from:, since:, -filter:replies).
        query: String,
        /// Jumlah maksimum hasil.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Urutkan menurut yang paling rame/relevan (bukan terbaru).
        #[arg(long)]
        top: bool,
    },
    /// Topik yang sedang ramai di X.
    Trends {
        #[arg(long, value_enum, default_value_t = TrendCategoryArg::Trending)]
        category: TrendCategoryArg,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Ambil timeline.
    Timeline {
        #[arg(long, value_enum, default_value_t = TimelineArg::Home)]
        kind: TimelineArg,
        /// Wajib untuk `--kind user`.
        #[arg(long)]
        handle: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Lihat profil pengguna.
    User {
        #[arg(long)]
        handle: String,
    },
    /// Tulis tweet.
    Post {
        /// Isi tweet. Kosongkan bila memakai --file.
        text: Option<String>,
        /// Baca isi dari berkas.
        #[arg(long)]
        file: Option<std::path::PathBuf>,
        /// Lampirkan media (dapat diulang).
        #[arg(long = "media")]
        media: Vec<std::path::PathBuf>,
        /// Balas ke tweet tertentu.
        #[arg(long)]
        reply_to: Option<String>,
    },
    /// Periksa kesehatan session, cookie, dan konfigurasi.
    Doctor,
    /// Ambil metrics sebuah tweet (dan simpan snapshot).
    Metrics {
        /// ID tweet.
        tweet_id: String,
        /// Tampilkan riwayat snapshot lokal.
        #[arg(long)]
        history: bool,
        /// Tampilkan selisih antara snapshot terlama dan terbaru.
        #[arg(long)]
        delta: bool,
        /// Jumlah maksimum snapshot yang dibaca.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Kelola draft.
    Draft {
        #[command(subcommand)]
        action: DraftAction,
    },
    /// Kelola job terjadwal.
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },
    /// Jalankan daemon scheduler.
    Run {
        /// Jalankan satu putaran lalu keluar.
        #[arg(long)]
        once: bool,
        /// Interval antar putaran (detik).
        #[arg(long, default_value_t = 30)]
        interval: u64,
    },
    /// Kelola cache dan discovery query ID GraphQL.
    Queries {
        #[command(subcommand)]
        action: QueriesAction,
    },
    /// Perintah tingkat rendah untuk inspeksi dan pembuatan fixture.
    Debug {
        #[command(subcommand)]
        action: DebugAction,
    },
    /// Inti C++: config tervalidasi, selftest, validasi profil, gambar frame.
    Native {
        #[command(subcommand)]
        action: NativeAction,
    },
    /// Periksa koherensi identitas profil penyamaran (+ header hasil derivasi).
    Anticheck {
        /// Berkas JSON profil, atau `-` untuk stdin. Tanpa ini, profil kosong.
        #[arg(long)]
        profile: Option<String>,
        /// OS yang diklaim identitas: lin, win, atau mac.
        #[arg(long, default_value = "lin")]
        target_os: String,
        /// Perbaiki dan cetak JSON hasil perbaikan.
        #[arg(long)]
        apply: bool,
        /// Cetak header HTTP hasil derivasi profil (hanya bila --profile ada).
        #[arg(long)]
        headers: bool,
    },
}

#[derive(Subcommand, Debug)]
enum DraftAction {
    /// Simpan draft baru (origin = human).
    New {
        /// Isi draft.
        #[arg(long)]
        body: String,
        /// Nama pendek untuk dikenali.
        #[arg(long)]
        name: Option<String>,
        /// Lampirkan media (dapat diulang).
        #[arg(long = "media")]
        media: Vec<std::path::PathBuf>,
    },
    /// Minta teks ke provider LLM lalu simpan sebagai draft.
    Generate {
        /// Prompt untuk model.
        #[arg(long)]
        prompt: String,
        /// Nama pendek untuk dikenali.
        #[arg(long)]
        name: Option<String>,
    },
    /// Daftar draft tersimpan.
    List {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Tampilkan satu draft.
    Show {
        /// ID draft.
        draft_id: String,
    },
    /// Posting draft (aksi eksplisit).
    Post {
        /// ID draft.
        draft_id: String,
    },
    /// Hapus draft.
    Delete {
        /// ID draft.
        draft_id: String,
    },
}

#[derive(Subcommand, Debug)]
enum ScheduleAction {
    /// Jadwalkan posting.
    Post {
        /// Waktu eksekusi (RFC3339), contoh 2026-10-01T09:00:00+07:00.
        #[arg(long)]
        at: String,
        /// Isi tweet. Salah satu dari --text/--file/--draft wajib.
        #[arg(long)]
        text: Option<String>,
        /// Baca isi dari berkas.
        #[arg(long)]
        file: Option<std::path::PathBuf>,
        /// Ambil isi dari draft tersimpan.
        #[arg(long)]
        draft: Option<String>,
        /// Lampirkan media (dapat diulang).
        #[arg(long = "media")]
        media: Vec<std::path::PathBuf>,
    },
    /// Daftar job terjadwal.
    List {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Batalkan job terjadwal.
    Cancel {
        /// ID job.
        job_id: String,
        /// Wajib: konfirmasi pembatalan.
        #[arg(long)]
        confirm: bool,
    },
}

#[derive(Subcommand, Debug)]
enum QueriesAction {
    /// Tampilkan semua query ID yang diketahui saat ini.
    List,
    /// Jalankan discovery dari bundle web X untuk memperbarui query ID.
    Refresh,
}

#[derive(Subcommand, Debug)]
enum DebugAction {
    /// Panggil endpoint GraphQL dan cetak JSON mentah.
    Raw {
        /// Nama operasi GraphQL (contoh: SearchTimeline).
        #[arg(long)]
        op: String,
        /// Variabel dalam format JSON string.
        #[arg(long, default_value = "{}")]
        vars: String,
    },
}

#[derive(Subcommand, Debug)]
enum NativeAction {
    /// Tampilkan config native yang sudah divalidasi (JSON).
    Config,
    /// Jalankan seluruh jalur inti C++ (percentile, text_width, gambar, profil).
    Selftest,
    /// Validasi berkas profil penyamaran.
    Profile {
        /// Berkas JSON profil.
        file: String,
    },
    /// Gambar teks ke berkas BMP 24-bit.
    Draw {
        /// Teks yang digambar.
        #[arg(long, default_value = "")]
        text: String,
        /// Berkas keluaran. Boleh kosong bila hanya ingin melihat header.
        #[arg(long, default_value = "")]
        out: String,
        /// Perbesaran tiap piksel glyph.
        #[arg(long, default_value_t = 2)]
        scale: u32,
        /// Cetak header hasil derivasi profil (`XHL_CONFIG.profile`), lalu keluar.
        #[arg(long)]
        headers: bool,
    },
}

#[derive(Subcommand, Debug)]
enum AuthAction {
    /// Impor cookie sesi dari berkas atau stdin.
    Import {
        /// Berkas cookie (Netscape/JSON/string). Tanpa ini, dibaca dari stdin.
        #[arg(long)]
        from: Option<std::path::PathBuf>,
    },
    /// Tampilkan status sesi akun.
    Status,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum TrendCategoryArg {
    Trending,
    News,
    Sport,
    Entertainment,
}

impl From<TrendCategoryArg> for xhl_core::domain::TrendCategory {
    fn from(v: TrendCategoryArg) -> Self {
        use xhl_core::domain::TrendCategory;
        match v {
            TrendCategoryArg::Trending => TrendCategory::Trending,
            TrendCategoryArg::News => TrendCategory::News,
            TrendCategoryArg::Sport => TrendCategory::Sport,
            TrendCategoryArg::Entertainment => TrendCategory::Entertainment,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum TimelineArg {
    Home,
    HomeLatest,
    User,
}

impl From<TimelineArg> for TimelineKind {
    fn from(v: TimelineArg) -> Self {
        match v {
            TimelineArg::Home => TimelineKind::Home,
            TimelineArg::HomeLatest => TimelineKind::HomeLatest,
            TimelineArg::User => TimelineKind::User,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();

    match run(cli).await {
        // `doctor` mengembalikan exit code sendiri: 0 sehat, 1 ada komponen gagal.
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            // Error selalu ke stderr dengan pesan yang aman dibaca pengguna.
            eprintln!("error: {e}");
            if e.needs_human() {
                eprintln!("petunjuk: jalankan `xhl doctor` untuk diagnosis.");
            }
            ExitCode::FAILURE
        }
    }
}

/// Inisialisasi logging. Filter dari `XHL_LOG` (default `warn`), keluaran ke stderr
/// agar stdout tetap bersih untuk data (`--json`).
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_env("XHL_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    // Kegagalan inisialisasi (mis. dipanggil dua kali) tidak boleh menghentikan CLI.
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

async fn run(cli: Cli) -> Result<u8, XhlError> {
    let mut config = Config::from_env()?;
    // Binding lokal agar pemakaian di bawah tetap ringkas; `cli` sendiri
    // hanya perlu hidup sampai titik ini (argumen sudah disalin).
    let json = cli.json;
    let driver_kind = cli.driver;
    if let Some(account) = cli.account {
        config.account = account;
    }

    let needs_store = matches!(
        cli.command,
        Command::Auth { .. }
            | Command::Doctor
            | Command::Post { .. }
            | Command::Queries { .. }
            | Command::Metrics { .. }
            | Command::Draft { .. }
            | Command::Schedule { .. }
            | Command::Run { .. }
    ) || driver_kind == DriverArg::Graphql;

    // Direktori data harus ada sebelum store dibuka, kalau tidak SQLite gagal
    // dengan pesan membingungkan pada instalasi baru.
    if needs_store {
        config.ensure_data_dir()?;
    }
    let store = needs_store
        .then(|| StoreHandle::open(&config.db_path()))
        .transpose()?;

    let cookies = match &store {
        Some(handle) => {
            let account = config.account.clone();
            handle.run(move |s| s.cookies(&account)).await?
        }
        None => None,
    };

    let ctx = xhl_core::driver::BuildContext {
        cookies,
        profile: config.http_profile,
        account: config.account.clone(),
        store: store.clone(),
        no_wait: config.no_wait,
    };
    let driver_build = driver_kind.kind().build(ctx);
    // Pesan kegagalan pembangunan driver dipakai `doctor`; cabang lain tetap
    // memperlakukannya sebagai error biasa.
    let build_error: Option<XhlError> = driver_build.as_ref().err().map(|e| match e {
        XhlError::AuthExpired(m) => XhlError::AuthExpired(m.clone()),
        _ => XhlError::Config(
            driver_build
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default(),
        ),
    });
    let driver = driver_build;
    match cli.command {
        Command::Auth { action } => auth(
            &config,
            store.as_ref().expect("auth memerlukan store"),
            action,
        )
        .await
        .map(|_| 0),
        Command::Doctor => {
            let code = doctor(
                &config,
                store.as_ref(),
                driver.as_ref().map(|b| &**b).ok(),
                build_error.as_ref(),
                json,
            )
            .await?;
            Ok(code)
        }
        Command::Search { query, limit, top } => {
            let svc = ResearchService::new(driver?);
            let ranking = if top {
                xhl_core::domain::SearchRanking::Top
            } else {
                xhl_core::domain::SearchRanking::Latest
            };
            let tweets = svc.search_ranked(&query, limit, ranking).await?;
            print_tweets(&tweets, json).map(|_| 0)
        }
        Command::Timeline {
            kind,
            handle,
            limit,
        } => {
            let svc = ResearchService::new(driver?);
            let handle = handle.as_deref().map(Handle::parse);
            let page = svc
                .timeline(kind.into(), handle.as_ref(), None, limit)
                .await?;
            print_tweets(&page.items, json).map(|_| 0)
        }
        Command::User { handle } => {
            let svc = ResearchService::new(driver?);
            let profile = svc.user(&Handle::parse(&handle)).await?;
            if json {
                print_json(&profile)?;
            } else {
                println!("{} ({})", profile.display_name, profile.handle);
                println!("id: {}", profile.id);
                if let Some(bio) = &profile.bio {
                    println!("bio: {bio}");
                }
                if let Some(f) = profile.followers {
                    println!("followers: {f}");
                }
            }
            Ok(0)
        }
        Command::Post {
            text,
            file,
            media,
            reply_to,
        } => {
            let body = match file {
                // `--file -` membaca stdin (helper yang menanganinya).
                Some(path) => xhl_core::service::post::read_content_file(&path)?,
                None => text.ok_or_else(|| {
                    XhlError::Invalid("berikan teks postingan atau --file".into())
                })?,
            };

            let media = media
                .into_iter()
                .map(xhl_core::domain::MediaInput::from_path)
                .collect();
            let post = Post {
                text: body,
                media,
                reply_to: reply_to.map(|id| id.into()),
            };

            let svc = match &store {
                Some(st) => PostService::with_store(driver?, st.clone(), config.account.clone()),
                None => PostService::new(driver?),
            };
            let posted = svc.post(post).await?;
            if json {
                print_json(&posted)?;
            } else {
                println!("terkirim: {}", posted.url);
                println!("id: {}", posted.id);
            }
            Ok(0)
        }
        Command::Trends { category, limit } => {
            let svc = ResearchService::new(driver?);
            let trends = svc.trends(category.into(), limit).await?;

            if json {
                print_json(&trends)?;
                return Ok(0);
            }
            if trends.is_empty() {
                println!("tidak ada trend");
                return Ok(0);
            }
            for (i, t) in trends.iter().enumerate() {
                // X kadang tidak mengirim peringkat; tampilkan urutannya.
                let rank = t.rank.unwrap_or((i + 1) as u32);
                let ctx = t.context.as_deref().unwrap_or("");
                let meta = t.meta_description.as_deref().unwrap_or("");
                println!("{rank:>2}. {}  {ctx}  {meta}", t.name);
            }
            Ok(0)
        }
        Command::Queries { action } => {
            let client = xhl_core::http::Client::new(config.http_profile)?;
            let store_for_reg = store.clone();
            let reg = xhl_core::query::QueryRegistry::new(client, store_for_reg);

            // Discovery butuh cookie sesi; tanpa itu halaman X yang diterima
            // adalah versi anonim yang tidak memuat operasi apa pun.
            if let Some(st) = &store {
                let account = config.account.clone();
                if let Ok(Some(cookies)) = st.run(move |s| s.cookies(&account)).await {
                    reg.set_discover_headers(
                        xhl_core::antibot::TransactionSigner::page_headers_for(
                            &cookies,
                            config.http_profile,
                        ),
                    );
                }
            }
            match action {
                QueriesAction::List => {
                    let list = reg.known().await;
                    println!("Daftar query ID ({} operasi):", list.len());
                    for (op, id) in list {
                        println!("  {op:35} {id}");
                    }
                    Ok(0)
                }
                QueriesAction::Refresh => {
                    let count = reg.refresh().await?;
                    println!("Discovery selesai: {count} operasi diperbarui.");
                    Ok(0)
                }
            }
        }
        Command::Metrics {
            tweet_id,
            history,
            delta,
            limit,
        } => {
            let st = store
                .ok_or_else(|| XhlError::Config("metrics memerlukan database lokal".into()))?;
            let svc = AnalyticsService::new(driver?, st);
            let id = TweetId(tweet_id);

            if history {
                let rows = svc.history(&id, limit).await?;
                if json {
                    let payload: Vec<serde_json::Value> = rows
                        .iter()
                        .map(|s| {
                            serde_json::json!({
                                "captured_at": s.captured_at,
                                "likes": s.metrics.likes,
                                "reposts": s.metrics.reposts,
                                "replies": s.metrics.replies,
                                "views": s.metrics.views,
                                "bookmarks": s.metrics.bookmarks,
                            })
                        })
                        .collect();
                    print_json(&payload)?;
                    return Ok(0);
                }
                if rows.is_empty() {
                    println!("belum ada snapshot untuk tweet {id}");
                    return Ok(0);
                }
                for s in &rows {
                    println!("{}", analytics::snapshot_line(s));
                }
                return Ok(0);
            }

            if delta {
                match svc.delta(&id, limit).await? {
                    Some(d) => {
                        if json {
                            print_json(&serde_json::json!({
                                "likes": d.likes,
                                "reposts": d.reposts,
                                "replies": d.replies,
                                "views": d.views,
                                "bookmarks": d.bookmarks,
                                "span_secs": d.span.as_secs(),
                            }))?;
                            return Ok(0);
                        }
                        println!("delta ({}s):", d.span.as_secs());
                        println!("  likes     : {:+}", d.likes);
                        println!("  reposts   : {:+}", d.reposts);
                        println!("  replies   : {:+}", d.replies);
                        println!("  views     : {:+}", d.views);
                        println!("  bookmarks : {:+}", d.bookmarks);
                        Ok(0)
                    }
                    None => {
                        println!(
                            "butuh minimal 2 snapshot; jalankan `xhl metrics {id}` lagi nanti"
                        );
                        Ok(0)
                    }
                }
            } else {
                let m = svc.capture(&id).await?;
                let count = svc.snapshot_count(&id).await?;
                if json {
                    print_json(&serde_json::json!({
                        "tweet_id": id.0,
                        "likes": m.likes,
                        "reposts": m.reposts,
                        "replies": m.replies,
                        "views": m.views,
                        "bookmarks": m.bookmarks,
                        "snapshots": count,
                    }))?;
                    return Ok(0);
                }
                let f = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_else(|| "-".to_owned());
                println!("tweet  : {}", id.0);
                println!("likes  : {}", f(m.likes));
                println!("reposts: {}", f(m.reposts));
                println!("replies: {}", f(m.replies));
                println!("views  : {}", f(m.views));
                println!("bookmks: {}", f(m.bookmarks));
                println!("snapshot tersimpan: {count}");
                Ok(0)
            }
        }
        Command::Draft { action } => {
            let st =
                store.ok_or_else(|| XhlError::Config("draft memerlukan database lokal".into()))?;
            draft_command(&config, st, driver?, action, json)
                .await
                .map(|_| 0)
        }
        Command::Schedule { action } => {
            let st = store
                .ok_or_else(|| XhlError::Config("schedule memerlukan database lokal".into()))?;
            schedule_command(&config, st, driver?, action, json)
                .await
                .map(|_| 0)
        }
        Command::Run { once, interval } => {
            let st =
                store.ok_or_else(|| XhlError::Config("run memerlukan database lokal".into()))?;
            let poster = PostService::with_store(driver?, st.clone(), config.account.clone());
            let sched = Scheduler::new(poster, st, config.account.clone());

            if once {
                let n = sched.tick().await?;
                if json {
                    print_json(&serde_json::json!({ "processed": n }))?;
                    return Ok(0);
                }
                println!("{n} job diproses");
                return Ok(0);
            }

            let (tx, rx) = tokio::sync::watch::channel(false);
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    let _ = tx.send(true);
                }
            });
            println!(
                "scheduler berjalan (interval {}s, akun {}); Ctrl-C untuk berhenti",
                interval, config.account
            );
            sched
                .run(std::time::Duration::from_secs(interval.max(1)), rx)
                .await
                .map(|_| 0)
        }
        Command::Native { action } => native(action, json),
        Command::Anticheck {
            profile,
            target_os,
            apply,
            headers,
        } => anticheck(profile.as_deref(), &target_os, apply, headers, json),
        Command::Debug { action } => match action {
            DebugAction::Raw { op, vars } => {
                let d = driver?;
                let vars_val: serde_json::Value = serde_json::from_str(&vars)
                    .map_err(|e| XhlError::Invalid(format!("vars bukan JSON valid: {e}")))?;
                let res = d.raw_query(&op, vars_val).await?;
                print_json(&res).map(|_| 0)
            }
        },
    }
}

/// Handler `xhl native`.
fn native(action: NativeAction, json: bool) -> Result<u8, XhlError> {
    match action {
        NativeAction::Config => {
            let cfg = xhl_core::native::NativeConfig::load()?;
            if let Some(err) = cfg.load_error() {
                // Config ada tetapi tidak sah: dilaporkan, bukan ditelan.
                eprintln!("peringatan: {err}");
            }
            // Kunci bertipe salah tidak menggagalkan pemuatan (config kosong
            // tetap berguna), tetapi harus terlihat: kalau tidak, nilai itu
            // diabaikan diam-diam dan perilaku xhl tampak "tidak berubah".
            let bad = cfg.validate_types()?;
            if !bad.is_empty() {
                return Err(XhlError::Config(format!(
                    "kunci config bertipe salah: {} (lihat `xhl native config --json` untuk isinya)",
                    bad.join(", ")
                )));
            }
            let dump = cfg.dump()?;
            if json {
                println!("{dump}");
            } else {
                println!("config native ({} kunci dikenal):", cfg.known_keys()?.len());
                println!("{dump}");
            }
            Ok(0)
        }
        NativeAction::Selftest => {
            use xhl_core::native::util;
            let samples: Vec<u64> = (1..=100).collect();
            let p50 = util::percentile(&samples, 0.5)?;
            if (p50 - 50.5).abs() > 1e-9 {
                return Err(XhlError::Internal(format!(
                    "selftest percentile gagal: p50 = {p50}, diharapkan 50.5"
                )));
            }
            let cols = util::text_width("ABC")?;
            if cols != 18 {
                return Err(XhlError::Internal(format!(
                    "selftest text_width gagal: 'ABC' = {cols} kolom, diharapkan 18"
                )));
            }
            let out = std::env::temp_dir().join("xhl-native-selftest.bmp");
            util::draw_frame("xhl OK", &out, 2)?;
            let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            if size == 0 {
                return Err(XhlError::Internal(
                    "selftest gambar gagal: BMP kosong".into(),
                ));
            }
            let _ = std::fs::remove_file(&out);

            // Profil valid harus lolos; profil rusak harus ditolak.
            let ok_profile = r#"{"navigator.userAgent":"UA","screen.width":1920}"#;
            xhl_core::native::ProfileHandle::validate(ok_profile)?;
            let bad_profile = r#"{"voices":[{"lang":"en"}]}"#;
            if xhl_core::native::ProfileHandle::validate(bad_profile).is_ok() {
                return Err(XhlError::Internal(
                    "selftest profil gagal: voices tidak lengkap seharusnya ditolak".into(),
                ));
            }

            if json {
                let out = serde_json::json!({
                    "native_version": xhl_core::native::ffi::version(),
                    "percentile_p50": p50,
                    "text_width_abc": cols,
                    "profile_validation": "ok",
                });
                print_json(&out)?;
            } else {
                println!("native    : {}", xhl_core::native::ffi::version());
                println!("percentile: p50([1..100]) = {p50}");
                println!("text_width: 'ABC' = {cols} kolom");
                println!("gambar    : BMP ditulis lalu dihapus");
                println!("profil    : validasi menerima yang sah, menolak voices separuh");
                println!("status    : ok");
            }
            Ok(0)
        }
        NativeAction::Profile { file } => {
            let raw = std::fs::read_to_string(&file).map_err(|e| {
                XhlError::Invalid(format!("profil tidak dapat dibaca ({file}): {e}"))
            })?;
            match xhl_core::native::ProfileHandle::validate(&raw) {
                Ok(()) => {
                    if json {
                        print_json(&serde_json::json!({ "valid": true, "path": file }))?;
                    } else {
                        println!("profil valid: {file}");
                    }
                    Ok(0)
                }
                Err(e) => Err(e),
            }
        }
        NativeAction::Draw {
            text,
            out,
            scale,
            headers,
        } => {
            // `--headers` menunjukkan identitas header yang benar-benar dipakai
            // transport (lewat `XHL_CONFIG.profile`), bukan argumen `--profile`
            // yang terpisah — tujuannya memverifikasi jalur produksi.
            if headers {
                match xhl_core::native::header_overrides() {
                    Some(h) => {
                        if json {
                            print_json(&serde_json::json!({
                                "user_agent": h.user_agent,
                                "accept_language": h.accept_language,
                                "accept_encoding": h.accept_encoding,
                                "order": h.order,
                                "from_navigator_fallback": h.user_agent_from_navigator,
                            }))?;
                        } else {
                            println!("urutan     : {}", h.order.join(", "));
                            println!(
                                "user-agent : {}",
                                h.user_agent.as_deref().unwrap_or("<bawaan xhl>")
                            );
                            println!(
                                "accept-lang: {}",
                                h.accept_language.as_deref().unwrap_or("<tidak diset>")
                            );
                            println!(
                                "accept-enc : {}",
                                h.accept_encoding.as_deref().unwrap_or("<tidak diset>")
                            );
                        }
                    }
                    None => {
                        if json {
                            print_json(&serde_json::json!({ "from_profile": false }))?;
                        } else {
                            println!(
                                "tidak ada profil di XHL_CONFIG.profile; xhl memakai header bawaannya"
                            );
                        }
                    }
                }
                return Ok(0);
            }
            if out.is_empty() {
                return Err(XhlError::Invalid(
                    "berikan --out <berkas> atau --headers".into(),
                ));
            }
            let path = std::path::PathBuf::from(&out);
            xhl_core::native::util::draw_frame(&text, &path, scale)?;
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if json {
                print_json(&serde_json::json!({ "path": out, "bytes": size }))?;
            } else {
                println!("frame ditulis: {out} ({size} byte)");
            }
            Ok(0)
        }
    }
}

/// Baca profil dari berkas atau stdin (`-`).
fn read_profile_arg(profile: Option<&str>) -> Result<Option<String>, XhlError> {
    use std::io::Read;
    let Some(p) = profile else { return Ok(None) };
    if p == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| XhlError::Invalid(format!("gagal membaca stdin: {e}")))?;
        return Ok(Some(buf));
    }
    let raw = std::fs::read_to_string(p)
        .map_err(|e| XhlError::Invalid(format!("profil tidak dapat dibaca ({p}): {e}")))?;
    Ok(Some(raw))
}

/// Handler `xhl anticheck`.
fn anticheck(
    profile: Option<&str>,
    target_os: &str,
    apply: bool,
    headers: bool,
    json: bool,
) -> Result<u8, XhlError> {
    if !matches!(target_os, "lin" | "win" | "mac") {
        return Err(XhlError::Invalid(format!(
            "target-os tidak dikenal: '{target_os}'; pakai lin, win, atau mac"
        )));
    }
    let raw = read_profile_arg(profile)?.unwrap_or_else(|| "{}".to_owned());

    // Header hanya bermakna bila ada profil yang benar-benar menyumbang header.
    if headers {
        match xhl_core::native::coherence::headers_from_profile(&raw)? {
            Some(h) => {
                if json {
                    print_json(&serde_json::json!({
                        "user_agent": h.user_agent,
                        "accept_language": h.accept_language,
                        "accept_encoding": h.accept_encoding,
                        "accept": h.accept,
                        "order": h.order,
                        "from_profile": h.from_profile,
                    }))?;
                } else {
                    println!("urutan     : {}", h.order.join(", "));
                    println!(
                        "user-agent : {}",
                        h.user_agent.as_deref().unwrap_or("<bawaan xhl>")
                    );
                    println!(
                        "accept-lang: {}",
                        h.accept_language.as_deref().unwrap_or("<tidak diset>")
                    );
                    println!(
                        "accept-enc : {}",
                        h.accept_encoding.as_deref().unwrap_or("<tidak diset>")
                    );
                }
            }
            None => {
                if json {
                    print_json(&serde_json::json!({ "from_profile": false }))?;
                } else {
                    println!("profil tidak menyetel header apa pun; xhl memakai header bawaannya");
                }
            }
        }
        return Ok(0);
    }

    if apply {
        let report = xhl_core::native::coherence::apply_coherence(&raw, target_os)?;
        if json {
            print_json(&serde_json::json!({
                "profile": serde_json::from_str::<serde_json::Value>(&report.json)
                    .unwrap_or(serde_json::Value::Null),
                "violations": report.violations,
            }))?;
        } else {
            println!("{}", report.json);
            for v in &report.violations {
                eprintln!("tersisa [{}] {}", v.rule, v.detail);
            }
        }
        return Ok(if report.violations.is_empty() { 0 } else { 1 });
    }

    let violations = xhl_core::native::coherence::check_profile(&raw, target_os)?;
    if json {
        print_json(&serde_json::json!({
            "target_os": target_os,
            "violations": violations,
            "coherent": violations.is_empty(),
        }))?;
    } else if violations.is_empty() {
        println!("identitas koheren untuk target-os {target_os}");
    } else {
        for v in &violations {
            println!("[{}] {}", v.rule, v.detail);
        }
    }
    Ok(if violations.is_empty() { 0 } else { 1 })
}

/// Susun provider LLM dari env; `NullProvider` bila tidak dikonfigurasi.
fn llm_provider(config: &Config) -> Box<dyn LlmProvider> {
    match OpenAiCompatProvider::from_env(config.http_profile) {
        Ok(Some(p)) => {
            tracing::info!(model = %p.model(), "provider LLM aktif");
            Box::new(p)
        }
        Ok(None) => Box::new(NullProvider),
        Err(e) => {
            tracing::warn!(error = %e, "gagal menyiapkan provider LLM; memakai NullProvider");
            Box::new(NullProvider)
        }
    }
}

/// Baca isi posting dari teks, berkas, atau draft tersimpan.
///
/// Membaca draft langsung dari store: tidak perlu membangun `DraftService`
/// (dan karenanya tidak perlu driver) hanya untuk mengambil teks.
async fn resolve_body(
    store: &StoreHandle,
    text: Option<String>,
    file: Option<std::path::PathBuf>,
    draft_id: Option<String>,
) -> Result<(String, Vec<xhl_core::domain::MediaInput>), XhlError> {
    if let Some(id) = draft_id {
        let key = id.clone();
        let row = store.run(move |s| s.get_draft(&key)).await?;
        let d = row.ok_or_else(|| XhlError::Invalid(format!("draft '{id}' tidak ditemukan")))?;
        let media: Vec<xhl_core::domain::MediaInput> = serde_json::from_str(&d.media_json)
            .map_err(|e| XhlError::Invalid(format!("media draft rusak: {e}")))?;
        return Ok((d.body, media));
    }

    if let Some(path) = file {
        return Ok((
            xhl_core::service::post::read_content_file(&path)?,
            Vec::new(),
        ));
    }

    text.map(|t| (t, Vec::new()))
        .ok_or_else(|| XhlError::Invalid("berikan --text, --file, atau --draft".into()))
}

async fn draft_command<D: xhl_core::driver::XWriter>(
    config: &Config,
    store: StoreHandle,
    driver: D,
    action: DraftAction,
    json: bool,
) -> Result<(), XhlError> {
    let poster = PostService::with_store(driver, store.clone(), config.account.clone());
    let svc = DraftService::new(store, llm_provider(config), config.account.clone(), poster);

    match action {
        DraftAction::New { body, name, media } => {
            let media = media
                .into_iter()
                .map(xhl_core::domain::MediaInput::from_path)
                .collect();
            let d = svc.create(name, body, media).await?;
            if json {
                return print_json(&serde_json::json!({
                    "id": d.id, "name": d.name, "origin": d.origin, "body": d.body,
                }));
            }
            println!("draft tersimpan: {}", d.id);
            Ok(())
        }
        DraftAction::Generate { prompt, name } => {
            let d = svc.generate(&prompt, name, GenOpts::default()).await?;
            if json {
                return print_json(&serde_json::json!({
                    "id": d.id, "name": d.name, "origin": d.origin, "body": d.body,
                }));
            }
            println!("draft dari LLM ({}): {}", svc.llm_name(), d.id);
            println!("---");
            println!("{}", d.body);
            println!("---");
            println!("belum diposting. gunakan: xhl draft post {}", d.id);
            Ok(())
        }
        DraftAction::List { limit } => {
            let rows = svc.list(limit).await?;
            if json {
                let payload: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "id": d.id, "name": d.name, "origin": d.origin, "body": d.body,
                        })
                    })
                    .collect();
                return print_json(&payload);
            }
            if rows.is_empty() {
                println!("belum ada draft");
                return Ok(());
            }
            for d in &rows {
                println!("{}", draft_mod::draft_line(d));
            }
            Ok(())
        }
        DraftAction::Show { draft_id } => {
            let d = svc
                .get(&draft_id)
                .await?
                .ok_or_else(|| XhlError::Invalid(format!("draft '{draft_id}' tidak ditemukan")))?;
            if json {
                return print_json(&serde_json::json!({
                    "id": d.id, "name": d.name, "origin": d.origin, "body": d.body,
                }));
            }
            println!("id     : {}", d.id);
            println!("nama   : {}", d.name.as_deref().unwrap_or("-"));
            println!("origin : {}", d.origin);
            println!("---");
            println!("{}", d.body);
            Ok(())
        }
        DraftAction::Post { draft_id } => {
            let posted = svc.post(&draft_id).await?;
            if json {
                return print_json(&posted);
            }
            println!("terkirim: {}", posted.url);
            println!("id: {}", posted.id);
            Ok(())
        }
        DraftAction::Delete { draft_id } => {
            let removed = svc.delete(&draft_id).await?;
            if !removed {
                return Err(XhlError::Invalid(format!(
                    "draft '{draft_id}' tidak ditemukan"
                )));
            }
            println!("draft dihapus: {draft_id}");
            Ok(())
        }
    }
}

async fn schedule_command<D: xhl_core::driver::XWriter>(
    config: &Config,
    store: StoreHandle,
    driver: D,
    action: ScheduleAction,
    json: bool,
) -> Result<(), XhlError> {
    let poster = PostService::with_store(driver, store.clone(), config.account.clone());
    let sched = Scheduler::new(poster, store.clone(), config.account.clone());

    match action {
        ScheduleAction::Post {
            at,
            text,
            file,
            draft,
            media,
        } => {
            let when = OffsetDateTime::parse(&at, &Rfc3339)
                .map_err(|e| XhlError::Invalid(format!("--at bukan RFC3339 valid ({at}): {e}")))?;

            let (body, mut resolved_media) = resolve_body(&store, text, file, draft).await?;
            resolved_media.extend(
                media
                    .into_iter()
                    .map(xhl_core::domain::MediaInput::from_path),
            );

            let post = Post {
                text: body,
                media: resolved_media,
                reply_to: None,
            };
            let key = xhl_core::service::post::dedup_key(&config.account, &post);
            let id = sched.schedule_post(when, post, Some(key)).await?;

            if json {
                return print_json(&serde_json::json!({ "job_id": id.to_string(), "run_at": at }));
            }
            println!("job terjadwal: {id}");
            println!("waktu: {at}");
            println!("jalankan `xhl run` agar tereksekusi");
            Ok(())
        }
        ScheduleAction::List { limit } => {
            let rows = sched.list(limit).await?;
            if json {
                let payload: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|j| {
                        serde_json::json!({
                            "id": j.id,
                            "run_at": j.run_at,
                            "state": j.state,
                            "attempts": j.attempts,
                        })
                    })
                    .collect();
                return print_json(&payload);
            }
            if rows.is_empty() {
                println!("belum ada job terjadwal");
                return Ok(());
            }
            for j in &rows {
                println!(
                    "{}  {}  {}  attempts={}",
                    j.id,
                    j.run_at.format(&Rfc3339).unwrap_or_default(),
                    j.state,
                    j.attempts
                );
            }
            Ok(())
        }
        ScheduleAction::Cancel { job_id, confirm } => {
            if !confirm {
                return Err(XhlError::Invalid(
                    "pembatalan butuh --confirm untuk mencegah kesalahan".into(),
                ));
            }
            let uuid = uuid::Uuid::parse_str(&job_id)
                .map_err(|e| XhlError::Invalid(format!("job id bukan UUID valid: {e}")))?;
            if !sched.cancel(&uuid).await? {
                return Err(XhlError::Invalid(format!(
                    "job '{job_id}' tidak ditemukan atau sudah final"
                )));
            }
            if json {
                return print_json(&serde_json::json!({ "cancelled": job_id }));
            }
            println!("job dibatalkan: {job_id}");
            Ok(())
        }
    }
}

async fn auth(config: &Config, store: &StoreHandle, action: AuthAction) -> Result<(), XhlError> {
    config.ensure_data_dir()?;
    match action {
        AuthAction::Import { from } => {
            let raw = match &from {
                Some(path) => std::fs::read_to_string(path).map_err(|e| {
                    XhlError::Invalid(format!("tidak bisa membaca {}: {e}", path.display()))
                })?,
                None => {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_to_string(&mut buf)
                        .map_err(|e| XhlError::Invalid(format!("gagal membaca stdin: {e}")))?;
                    buf
                }
            };

            let source = match &from {
                Some(p) => p.display().to_string(),
                None => "stdin".to_owned(),
            };
            let cookies = Cookies::parse_auto(&raw)?;
            let found = cookies.extras.len();
            let account = config.account.clone();
            let db = config.db_path();

            // Operasi blocking dijalankan di spawn_blocking oleh StoreHandle.
            let event_source = source.clone();
            store
                .run(move |s| {
                    s.upsert_cookies(&account, &cookies)?;
                    s.record_event(&account, "auth_import", &format!("sumber: {event_source}"))
                })
                .await?;

            println!("cookie tersimpan untuk akun '{}'", config.account);
            println!("sumber: {source} ({found} cookie tambahan)");
            println!("database: {}", db.display());
            Ok(())
        }
        AuthAction::Status => {
            let account = config.account.clone();
            let cookies = store.run(move |s| s.cookies(&account)).await?;
            match cookies {
                Some(c) => {
                    println!("akun: {}", config.account);
                    println!(
                        "cookie: ada (auth_token {}, ct0 {})",
                        redact(&c.auth_token),
                        redact(&c.ct0)
                    );
                    println!(
                        "catatan: jalankan `xhl doctor` untuk memverifikasi cookie masih berlaku."
                    );
                }
                None => {
                    println!("akun: {}", config.account);
                    println!("cookie: belum ada");
                    println!("impor dengan: xhl auth import --from cookies.txt");
                }
            }
            Ok(())
        }
    }
}

async fn doctor(
    config: &Config,
    store: Option<&StoreHandle>,
    driver: Option<&dyn XDriver>,
    build_error: Option<&XhlError>,
    json: bool,
) -> Result<u8, XhlError> {
    // Cookie dibaca hanya bila store tersedia; kegagalan baca dilaporkan, bukan
    // menggagalkan seluruh diagnosis.
    let cookies = match store {
        Some(st) => {
            let account = config.account.clone();
            match st.run(move |s| s.cookies(&account)).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "gagal membaca cookie dari store");
                    None
                }
            }
        }
        None => None,
    };
    let events = match store {
        Some(st) => {
            let account = config.account.clone();
            st.run(move |s| s.recent_events(&account, 5))
                .await
                .unwrap_or_default()
        }
        None => Vec::new(),
    };

    let cookie_state = if cookies.is_some() {
        "ada"
    } else {
        "belum ada"
    };

    // Probe jaringan boleh gagal: `doctor` ada justru untuk melaporkan kondisi,
    // termasuk saat X tidak dapat dijangkau.
    let (health, health_error) = match driver {
        Some(d) => match d.health().await {
            Ok(h) => (Some(h), None),
            Err(e) => (None, Some(e)),
        },
        None => (None, None),
    };

    let diag = match driver {
        Some(d) => d.diagnostics().await,
        None => None,
    };

    // Driver yang gagal dibangun adalah temuan, bukan alasan berhenti.
    let driver_note = if driver.is_some() {
        "tersedia".to_owned()
    } else if let Some(e) = build_error {
        format!("tidak dapat dibangun: {}", e.agent_safe_message())
    } else {
        "belum tersedia".to_owned()
    };

    let verdict = match (&cookies, &health, &health_error) {
        (None, _, _) => "impor cookie dulu: xhl auth import --from cookies.txt".to_owned(),
        (Some(_), Some(SessionStatus::Ok { .. }), _) => "siap".to_owned(),
        (Some(_), Some(SessionStatus::Expired { .. }), _) => {
            "cookie tersimpan tetapi tidak berlaku — impor ulang dari browser".to_owned()
        }
        (Some(_), None, Some(XhlError::Network(_) | XhlError::Timeout { .. })) => {
            "cookie ada, tetapi X tidak dapat dijangkau dari sini — periksa jaringan/proxy"
                .to_owned()
        }
        (Some(_), None, Some(e)) => format!("probe sesi gagal: {}", e.agent_safe_message()),
        (Some(_), None, None) => "driver tidak dapat dibangun; lihat baris driver".to_owned(),
    };

    // Sehat hanya bila sesi benar-benar terverifikasi.
    let healthy = matches!(health, Some(SessionStatus::Ok { .. }));

    // Operasi inti yang butuh discovery runtime tidak dihitung sebagai cacat;
    // ia ter-resolve saat request pertama.
    let runtime_only: &[&str] = &["HomeTimeline", "HomeLatestTimeline", "CreateTweet"];
    let missing_blocking: Vec<&str> = diag
        .as_ref()
        .map(|d| {
            d.missing_operations
                .iter()
                .copied()
                .filter(|op| !runtime_only.contains(op))
                .collect()
        })
        .unwrap_or_default();

    if json {
        let report = serde_json::json!({
            "account": config.account,
            "database": config.db_path().display().to_string(),
            "cookie": cookie_state,
            "profile": config.http_profile.name,
            "session": match &health {
                Some(SessionStatus::Ok { screen_name, .. }) => serde_json::json!({"state": "ok", "screen_name": screen_name}),
                Some(SessionStatus::Expired { detail }) => serde_json::json!({"state": "expired", "detail": detail}),
                None => serde_json::json!({
                    "state": "tidak diperiksa",
                    "detail": health_error.as_ref().map(|e| e.agent_safe_message()),
                }),
            },
            "signer": diag.as_ref().map(|d| if d.signer_ready { "ready" } else { "belum bootstrap" }),
            "queries": diag.as_ref().map(|d| serde_json::json!({
                "known": d.known_operations.len(),
                "missing_core": d.missing_operations,
                "missing_blocking": missing_blocking,
            })),
            "driver": driver_note,
            "healthy": healthy,
            "verdict": verdict,
            "recent_events": events,
        });
        print_json(&report)?;
    } else {
        println!("akun      : {}", config.account);
        println!("database  : {}", config.db_path().display());
        println!("cookie    : {cookie_state}");
        println!("profile   : {}", config.http_profile.name);
        println!(
            "session   : {}",
            match (&health, &health_error) {
                (Some(h), _) => describe_health(h),
                (None, Some(e)) => format!("tidak dapat diperiksa — {}", e.agent_safe_message()),
                (None, None) => "belum dapat diperiksa".to_owned(),
            }
        );
        if let Some(d) = &diag {
            let signer_str = if d.signer_ready {
                "ready"
            } else {
                "belum bootstrap (akan bootstrap otomatis saat request)"
            };
            println!("signer    : {signer_str}");
            println!("queries   : {} operasi diketahui", d.known_operations.len());
            if !d.missing_operations.is_empty() {
                println!("missing   : {}", d.missing_operations.join(", "));
            }
            if !missing_blocking.is_empty() {
                println!(
                    "perhatian : operasi wajib belum ter-resolve: {}",
                    missing_blocking.join(", ")
                );
            }
        }
        println!("driver    : {driver_note}");
        println!("verdict   : {verdict}");
        if !events.is_empty() {
            println!("event terakhir:");
            for e in &events {
                println!("  {e}");
            }
        }
    }

    Ok(if healthy { 0 } else { 1 })
}

fn describe_health(status: &SessionStatus) -> String {
    match status {
        SessionStatus::Ok { screen_name, .. } => format!("ok (@{screen_name})"),
        SessionStatus::Expired { detail } => format!("kedaluwarsa ({detail})"),
    }
}

fn print_tweets(tweets: &[xhl_core::domain::Tweet], json: bool) -> Result<(), XhlError> {
    if json {
        return print_json(&tweets);
    }
    if tweets.is_empty() {
        println!("tidak ada hasil");
        return Ok(());
    }
    for t in tweets {
        println!(
            "{}  {}",
            t.id,
            t.created_at.format(&Rfc3339).unwrap_or_default()
        );
        println!("  {}", t.text.replace('\n', " "));
        println!(
            "  {}",
            t.metrics
                .as_ref()
                .map(|m| m.summary())
                .unwrap_or_else(|| "tanpa metrik".to_owned())
        );
        println!("  {}", t.url);
    }
    Ok(())
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), XhlError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| XhlError::Internal(format!("serialisasi JSON: {e}")))?;
    println!("{text}");
    Ok(())
}

/// Tampilkan petunjuk panjang tanpa membocorkan nilai kredensial.
fn redact(secret: &str) -> String {
    let n = secret.chars().count();
    if n == 0 {
        "<kosong>".to_owned()
    } else {
        format!("{n} karakter")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_tidak_membocorkan_isi() {
        assert_eq!(redact("RAHASIA"), "7 karakter");
        assert!(!redact("RAHASIA").contains("RAHASIA"));
        assert_eq!(redact(""), "<kosong>");
    }

    #[test]
    fn graphql_tanpa_cookie_gagal_auth_expired() {
        let ctx = xhl_core::driver::BuildContext {
            cookies: None,
            profile: &xhl_core::http::headers::FIREFOX_133,
            account: "default".into(),
            store: None,
            no_wait: false,
        };
        match DriverKind::GraphQl.build(ctx) {
            Err(XhlError::AuthExpired(_)) => {}
            Err(other) => panic!("error tak terduga: {other}"),
            Ok(_) => panic!("driver GraphQL tanpa cookie seharusnya gagal"),
        }
    }
}
