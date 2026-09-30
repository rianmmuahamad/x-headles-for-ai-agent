//! Tipe domain. Murni data — tanpa I/O, tanpa pengetahuan protokol.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// ID tweet numerik dari X.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TweetId(pub String);

impl std::fmt::Display for TweetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for TweetId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for TweetId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Handle akun, tanpa `@`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Handle(pub String);

impl Handle {
    /// Buang `@` di depan bila ada.
    pub fn parse(raw: &str) -> Self {
        Self(raw.trim().trim_start_matches('@').to_owned())
    }
}

impl std::fmt::Display for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "@{}", self.0)
    }
}

/// ID akun internal kita (bukan ID X). Dipakai untuk profil & store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountId(pub String);

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Ringkasan akun tersimpan: identitas saja, tanpa kredensial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSummary {
    pub id: AccountId,
    pub handle: Option<Handle>,
    pub user_id: Option<String>,
}

/// Identitas akun hasil health-check. Tidak memuat kredensial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub handle: Option<Handle>,
    /// `rest_id` X — di-cache setelah `UserByScreenName`.
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    pub handle: Handle,
    pub display_name: String,
    pub verified: bool,
}

/// Profil pengguna X (hasil `UserByScreenName`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserProfile {
    /// `rest_id` X.
    pub id: String,
    pub handle: Handle,
    pub display_name: String,
    pub verified: bool,
    pub bio: Option<String>,
    pub followers: Option<u64>,
}

/// Snapshot hasil baca. Bukan input write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tweet {
    pub id: TweetId,
    pub author: Author,
    pub text: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub media: Vec<MediaItem>,
    pub metrics: Option<Metrics>,
    pub url: String,
}

/// Sebagian field bisa absen di respons X → semuanya `Option`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metrics {
    pub likes: Option<u64>,
    pub reposts: Option<u64>,
    pub replies: Option<u64>,
    pub views: Option<u64>,
    pub bookmarks: Option<u64>,
}

/// Media hasil baca (remote, sudah ada di X).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaItem {
    pub kind: MediaKind,
    pub url: String,
    pub thumbnail_url: Option<String>,
    pub alt: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Video,
    Gif,
}

/// Media yang akan diunggah (lokal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaInput {
    Image {
        path: std::path::PathBuf,
        alt: Option<String>,
    },
    Video {
        path: std::path::PathBuf,
        alt: Option<String>,
    },
}

impl MediaInput {
    /// Tentukan jenis media dari ekstensi berkas.
    ///
    /// Ini **satu-satunya** tempat pengetahuan tentang ekstensi berada; adapter
    /// tidak boleh mengklasifikasi sendiri.
    pub fn from_path(path: impl Into<std::path::PathBuf>) -> Self {
        let path = path.into();
        let is_video = matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("mp4" | "mov" | "webm" | "m4v")
        );
        if is_video {
            Self::Video { path, alt: None }
        } else {
            Self::Image { path, alt: None }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Image { .. } => "image",
            Self::Video { .. } => "video",
        }
    }

    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::Image { path, .. } | Self::Video { path, .. } => path,
        }
    }

    pub fn description(&self) -> String {
        format!("{}:{}", self.kind(), self.path().display())
    }
}

/// Input untuk menulis tweet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Post {
    pub text: String,
    pub media: Vec<MediaInput>,
    pub reply_to: Option<TweetId>,
}

impl Post {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            media: Vec::new(),
            reply_to: None,
        }
    }
}

/// Hasil write. `id` dibaca dari respons X, bukan diasumsikan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posted {
    pub id: TweetId,
    pub url: String,
    #[serde(with = "time::serde::rfc3339")]
    pub posted_at: OffsetDateTime,
}

/// Hasil pencarian/timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineKind {
    Home,
    HomeLatest,
    User,
}

/// Topik yang sedang ramai di X.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trend {
    pub name: String,
    /// Peringkat 1-based bila X mengirimkannya.
    pub rank: Option<u32>,
    /// Konteks domain, mis. "Sports · Trending".
    pub context: Option<String>,
    /// Keterangan tambahan dari X, sering memuat jumlah posting.
    pub meta_description: Option<String>,
    /// URL pencarian untuk topik ini.
    pub url: Option<String>,
}

/// Kategori daftar trend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrendCategory {
    Trending,
    News,
    Sport,
    Entertainment,
}

impl TrendCategory {
    /// Nama tunggal untuk tampilan/argumen CLI & MCP.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trending => "trending",
            Self::News => "news",
            Self::Sport => "sport",
            Self::Entertainment => "entertainment",
        }
    }
}

/// Urutan hasil pencarian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchRanking {
    /// Tweet terbaru.
    Latest,
    /// Tweet paling relevan/berinteraksi — inilah "yang rame".
    Top,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    pub limit: usize,
    pub cursor: Option<String>,
    pub ranking: SearchRanking,
}

impl SearchQuery {
    pub fn new(query: impl Into<String>, limit: usize) -> Self {
        Self {
            query: query.into(),
            limit,
            cursor: None,
            ranking: SearchRanking::Latest,
        }
    }

    /// Query dengan ranking tertentu.
    pub fn ranked(query: impl Into<String>, limit: usize, ranking: SearchRanking) -> Self {
        Self {
            ranking,
            ..Self::new(query, limit)
        }
    }
}

/// Angka untuk tampilan: `1.2k`, `2.5m`, atau `-` bila tidak ada.
pub fn format_count(v: Option<u64>) -> String {
    match v {
        None => "-".to_owned(),
        Some(n) if n < 1_000 => n.to_string(),
        Some(n) if n < 1_000_000 => format!("{:.1}k", n as f64 / 1_000.0),
        Some(n) => format!("{:.1}m", n as f64 / 1_000_000.0),
    }
}

impl Metrics {
    /// Ringkas metrik untuk ditampilkan. Hanya field yang ada yang muncul.
    pub fn summary(&self) -> String {
        if *self == Metrics::default() {
            return "tanpa metrik".to_owned();
        }
        let mut parts = Vec::new();
        if let Some(v) = self.likes {
            parts.push(format!("{} likes", format_count(Some(v))));
        }
        if let Some(v) = self.reposts {
            parts.push(format!("{} reposts", format_count(Some(v))));
        }
        if let Some(v) = self.replies {
            parts.push(format!("{} replies", format_count(Some(v))));
        }
        if let Some(v) = self.views {
            parts.push(format!("{} views", format_count(Some(v))));
        }
        if let Some(v) = self.bookmarks {
            parts.push(format!("{} bookmarks", format_count(Some(v))));
        }
        if parts.is_empty() {
            "tanpa metrik".to_owned()
        } else {
            parts.join(" · ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_parse_membuang_at() {
        assert_eq!(Handle::parse(" @jack ").0, "jack");
        assert_eq!(Handle::parse("jack").0, "jack");
        assert_eq!(Handle::parse("@jack").to_string(), "@jack");
    }

    #[test]
    fn format_count_membulatkan_ribuan_dan_jutaan() {
        assert_eq!(format_count(None), "-");
        assert_eq!(format_count(Some(0)), "0");
        assert_eq!(format_count(Some(999)), "999");
        assert_eq!(format_count(Some(1_200)), "1.2k");
        assert_eq!(format_count(Some(2_500_000)), "2.5m");
    }

    #[test]
    fn metrics_summary_hanya_menampilkan_yang_ada() {
        assert_eq!(Metrics::default().summary(), "tanpa metrik");

        let m = Metrics {
            likes: Some(12),
            views: Some(1_200),
            ..Default::default()
        };
        let s = m.summary();
        assert!(s.contains("12 likes"), "{s}");
        assert!(s.contains("1.2k views"), "{s}");
        assert!(
            !s.contains("reposts"),
            "field kosong tidak ditampilkan: {s}"
        );
    }

    #[test]
    fn search_query_ranking_default_latest() {
        assert_eq!(
            SearchQuery::new("rust", 5).ranking,
            SearchRanking::Latest,
            "perilaku lama tidak berubah"
        );
        assert_eq!(
            SearchQuery::ranked("rust", 5, SearchRanking::Top).ranking,
            SearchRanking::Top
        );
    }

    #[test]
    fn media_dari_ekstensi() {
        assert_eq!(MediaInput::from_path("/tmp/a.MP4").kind(), "video");
        assert_eq!(MediaInput::from_path("/tmp/a.mov").kind(), "video");
        assert_eq!(MediaInput::from_path("/tmp/a.jpg").kind(), "image");
        assert_eq!(
            MediaInput::from_path("/tmp/a").kind(),
            "image",
            "tanpa ekstensi = gambar"
        );
        assert_eq!(
            MediaInput::from_path("/tmp/a.jpg").path(),
            std::path::Path::new("/tmp/a.jpg")
        );
        assert_eq!(
            MediaInput::from_path("/tmp/a.jpg").description(),
            "image:/tmp/a.jpg"
        );
    }
}
