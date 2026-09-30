use std::sync::LazyLock;
use time::format_description::FormatItem;
use time::OffsetDateTime;

use crate::domain::{
    Author, Handle, MediaItem, MediaKind, Metrics, Trend, Tweet, TweetId, UserProfile,
};

static X_DATE_FORMAT: LazyLock<Vec<FormatItem<'static>>> = LazyLock::new(|| {
    time::format_description::parse_borrowed::<1>(
        "[weekday repr:short] [month repr:short] [day padding:space] [hour]:[minute]:[second] [offset_hour sign:mandatory][offset_minute] [year]",
    )
    .expect("format tanggal X valid")
});

pub fn parse_x_date(raw: &str) -> OffsetDateTime {
    let format = &*X_DATE_FORMAT;
    OffsetDateTime::parse(raw, format).unwrap_or_else(|e| {
        tracing::warn!(raw = %raw, error = %e, "gagal parse format tanggal X, fallback UNIX_EPOCH");
        OffsetDateTime::UNIX_EPOCH
    })
}

/// Bongkar kemungkinan pembungkus seperti `TweetWithVisibilityResults`
fn unwrap_tweet_node(value: &serde_json::Value) -> &serde_json::Value {
    let typename = value
        .get("__typename")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if typename == "TweetWithVisibilityResults" {
        if let Some(inner) = value.get("tweet") {
            return unwrap_tweet_node(inner);
        }
    }
    value
}

pub fn decode_tweet(raw: &serde_json::Value) -> Option<Tweet> {
    let node = unwrap_tweet_node(raw);

    let id_str = node.get("rest_id").and_then(|v| v.as_str()).or_else(|| {
        node.get("legacy")
            .and_then(|l| l.get("id_str"))
            .and_then(|v| v.as_str())
    })?;
    let id = TweetId(id_str.to_string());

    let legacy = node.get("legacy").unwrap_or(&serde_json::Value::Null);

    // Ambil teks: utamakan note_tweet untuk tweet panjang (Twitter Blue / X Premium)
    let text = node
        .get("note_tweet")
        .and_then(|nt| nt.get("note_tweet_results"))
        .and_then(|res| res.get("result"))
        .and_then(|res| res.get("text"))
        .and_then(|t| t.as_str())
        .or_else(|| legacy.get("full_text").and_then(|t| t.as_str()))
        .unwrap_or("")
        .to_string();

    // Tanggal
    let created_at = legacy
        .get("created_at")
        .and_then(|c| c.as_str())
        .map(parse_x_date)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);

    // Author: X memindahkan identitas ke `core.user_results.result.core`;
    // `legacy` pada simpul pengguna sudah tidak dikirim.
    let user_node = node
        .get("core")
        .and_then(|c| c.get("user_results"))
        .and_then(|ur| ur.get("result"))
        .or_else(|| node.get("user_results").and_then(|ur| ur.get("result")));

    let (handle, display_name, verified) = if let Some(u) = user_node {
        let core = u.get("core");
        let legacy = u.get("legacy");
        let pick = |key: &str| -> Option<&str> {
            core.and_then(|c| c.get(key))
                .and_then(|v| v.as_str())
                .or_else(|| legacy.and_then(|l| l.get(key)).and_then(|v| v.as_str()))
        };
        let verified = u
            .get("verification")
            .and_then(|v| v.get("verified"))
            .and_then(|v| v.as_bool())
            .or_else(|| {
                legacy
                    .and_then(|l| l.get("verified"))
                    .and_then(|v| v.as_bool())
            })
            .unwrap_or(false)
            || u.get("is_blue_verified")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

        (
            Handle::parse(pick("screen_name").unwrap_or("unknown")),
            pick("name").unwrap_or("Unknown").to_string(),
            verified,
        )
    } else {
        (Handle::parse("unknown"), "Unknown".to_string(), false)
    };

    let author = Author {
        handle: handle.clone(),
        display_name,
        verified,
    };

    // Media
    let mut media_items = Vec::new();
    if let Some(media_arr) = legacy
        .get("extended_entities")
        .and_then(|ee| ee.get("media"))
        .and_then(|m| m.as_array())
    {
        for m in media_arr {
            let m_type = m.get("type").and_then(|t| t.as_str()).unwrap_or("photo");
            let thumb = m
                .get("media_url_https")
                .and_then(|u| u.as_str())
                .map(|s| s.to_string());
            let alt = m
                .get("ext_alt_text")
                .and_then(|a| a.as_str())
                .map(|s| s.to_string());

            let (kind, url) = match m_type {
                "video" => {
                    // Cari varian bitrate tertinggi
                    let best_variant = m
                        .get("video_info")
                        .and_then(|vi| vi.get("variants"))
                        .and_then(|va| va.as_array())
                        .and_then(|variants| {
                            variants
                                .iter()
                                .filter(|v| {
                                    v.get("content_type").and_then(|ct| ct.as_str())
                                        == Some("video/mp4")
                                })
                                .max_by_key(|v| {
                                    v.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0)
                                })
                        })
                        .and_then(|v| v.get("url"))
                        .and_then(|u| u.as_str());

                    (
                        MediaKind::Video,
                        best_variant
                            .map(|s| s.to_string())
                            .or_else(|| thumb.clone())
                            .unwrap_or_default(),
                    )
                }
                "animated_gif" => {
                    let gif_url = m
                        .get("video_info")
                        .and_then(|vi| vi.get("variants"))
                        .and_then(|va| va.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|v| v.get("url"))
                        .and_then(|u| u.as_str());

                    (
                        MediaKind::Gif,
                        gif_url
                            .map(|s| s.to_string())
                            .or_else(|| thumb.clone())
                            .unwrap_or_default(),
                    )
                }
                _ => (MediaKind::Image, thumb.clone().unwrap_or_default()),
            };

            media_items.push(MediaItem {
                kind,
                url,
                thumbnail_url: thumb,
                alt,
            });
        }
    }

    // Metrics
    let views = node
        .get("views")
        .and_then(|v| v.get("count"))
        .and_then(|c| {
            if let Some(s) = c.as_str() {
                s.parse::<u64>().ok()
            } else {
                c.as_u64()
            }
        });

    let likes = legacy.get("favorite_count").and_then(|c| c.as_u64());
    let reposts = legacy.get("retweet_count").and_then(|c| c.as_u64());
    let replies = legacy.get("reply_count").and_then(|c| c.as_u64());
    let bookmarks = legacy.get("bookmark_count").and_then(|c| c.as_u64());

    let metrics = if views.is_some()
        || likes.is_some()
        || reposts.is_some()
        || replies.is_some()
        || bookmarks.is_some()
    {
        Some(Metrics {
            likes,
            reposts,
            replies,
            views,
            bookmarks,
        })
    } else {
        None
    };

    let url = format!("https://x.com/{}/status/{}", author.handle.0, id.0);

    Some(Tweet {
        id,
        author,
        text,
        created_at,
        media: media_items,
        metrics,
        url,
    })
}

pub fn decode_user(value: &serde_json::Value) -> Option<UserProfile> {
    // Cari simpul pengguna: `data.user.result`, atau langsung `result`.
    let result = value
        .get("user")
        .and_then(|u| u.get("result"))
        .or_else(|| value.get("result"))
        .unwrap_or(value);

    let rest_id = result.get("rest_id").and_then(|v| v.as_str())?.to_string();

    // X memindahkan identitas pengguna dari `legacy` ke `core`. Terima keduanya
    // agar tetap bekerja bila X mengubahnya lagi.
    let core = result.get("core");
    let legacy = result.get("legacy");

    let pick_str = |key: &str| -> Option<&str> {
        core.and_then(|c| c.get(key))
            .and_then(|v| v.as_str())
            .or_else(|| legacy.and_then(|l| l.get(key)).and_then(|v| v.as_str()))
    };

    let handle = Handle::parse(pick_str("screen_name")?);
    let display_name = pick_str("name").unwrap_or("").to_string();

    // Verifikasi: `verification.verified` (bentuk baru) atau `legacy.verified`,
    // ditambah `is_blue_verified`.
    let verified = result
        .get("verification")
        .and_then(|v| v.get("verified"))
        .and_then(|v| v.as_bool())
        .or_else(|| {
            legacy
                .and_then(|l| l.get("verified"))
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false)
        || result
            .get("is_blue_verified")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

    // Bio: `profile_bio.description` (bentuk baru) atau `legacy.description`.
    let bio = result
        .get("profile_bio")
        .and_then(|b| b.get("description"))
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            legacy
                .and_then(|l| l.get("description"))
                .and_then(|d| d.as_str())
                .map(|s| s.to_string())
        });

    // Followers: `relationship_counts.followers` (bentuk baru) atau `legacy`.
    let followers = result
        .get("relationship_counts")
        .and_then(|r| r.get("followers"))
        .and_then(|f| f.as_u64())
        .or_else(|| {
            legacy
                .and_then(|l| l.get("followers_count"))
                .and_then(|f| f.as_u64())
        });

    Some(UserProfile {
        id: rest_id,
        handle,
        display_name,
        verified,
        bio,
        followers,
    })
}

/// Ambil `instructions` dari sarang timeline apa pun.
///
/// X menyarangkan respons dengan cara berbeda per operasi:
/// `timeline.timeline.instructions` (UserTweets/TweetDetail/trends),
/// `timeline.instructions` (bentuk lama), atau daftar instructions langsung.
/// Helper ini menerima semuanya sehingga jalur pemanggil tidak perlu tahu.
pub fn instructions_of(node: &serde_json::Value) -> &serde_json::Value {
    if let Some(inner) = node.get("timeline").and_then(|t| t.get("instructions")) {
        return inner;
    }
    if let Some(inner) = node
        .get("timeline")
        .and_then(|t| t.get("timeline"))
        .and_then(|t| t.get("instructions"))
    {
        return inner;
    }
    if let Some(inner) = node.get("instructions") {
        return inner;
    }
    node
}

pub fn decode_entries(instructions: &serde_json::Value) -> (Vec<Tweet>, Option<String>) {
    let mut tweets = Vec::new();
    let mut next_cursor = None;

    let inst_array = if let Some(arr) = instructions.as_array() {
        arr.as_slice()
    } else if let Some(arr) = instructions.get("instructions").and_then(|i| i.as_array()) {
        arr.as_slice()
    } else {
        return (tweets, next_cursor);
    };

    for inst in inst_array {
        let entries: Vec<&serde_json::Value> =
            if let Some(arr) = inst.get("entries").and_then(|e| e.as_array()) {
                arr.iter().collect()
            } else if let Some(single) = inst.get("entry") {
                vec![single]
            } else {
                Vec::new()
            };

        for entry in entries {
            let entry_id = entry
                .get("entryId")
                .and_then(|id| id.as_str())
                .unwrap_or("");

            // Cek jika ini cursor
            if entry_id.starts_with("cursor-bottom") || entry_id.starts_with("cursor-next") {
                if let Some(cursor_val) = entry
                    .get("content")
                    .and_then(|c| {
                        c.get("value")
                            .or_else(|| c.get("itemContent").and_then(|ic| ic.get("value")))
                    })
                    .and_then(|v| v.as_str())
                {
                    next_cursor = Some(cursor_val.to_string());
                }
            }

            // Ambil tweet
            let tweet_result = entry
                .get("content")
                .and_then(|c| c.get("itemContent"))
                .and_then(|ic| ic.get("tweet_results"))
                .and_then(|tr| tr.get("result"));

            if let Some(res) = tweet_result {
                if let Some(tw) = decode_tweet(res) {
                    tweets.push(tw);
                }
            }
        }
    }

    (tweets, next_cursor)
}

/// Konversi angka yang bisa datang sebagai angka atau string.
fn to_u32(v: &serde_json::Value) -> Option<u32> {
    v.as_u64()
        .map(|n| n as u32)
        .or_else(|| v.as_str().and_then(|s| s.parse::<u32>().ok()))
}

/// Ambil teks dari beberapa jalur alternatif.
fn first_str<'a>(v: &'a serde_json::Value, paths: &[&[&str]]) -> Option<&'a str> {
    for path in paths {
        let mut cur = v;
        let mut ok = true;
        for key in *path {
            match cur.get(*key) {
                Some(next) => cur = next,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            if let Some(s) = cur.as_str() {
                return Some(s);
            }
        }
    }
    None
}

/// Ambil daftar trend dari respons `GenericTimelineById`.
///
/// Bentuk terverifikasi terhadap X (2026-09-30):
/// `timeline.timeline.instructions[].entries[].content.items[].item.itemContent`
/// dengan `__typename == "TimelineTrend"`. Perhatikan trend **tidak** berada di
/// `content.itemContent` (itu untuk tweet), melainkan di dalam `content.items`.
/// Konteks ramai ada di `social_context.text` (mis. "Trending now · Sports · 54 posts").
///
/// Entri tanpa nama dibuang; daftar kosong bukan kesalahan.
pub fn decode_trends(data: &serde_json::Value) -> Vec<Trend> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let instructions = data
        .get("timeline")
        .and_then(|t| t.get("timeline"))
        .and_then(|t| t.get("instructions"))
        .and_then(|i| i.as_array())
        .or_else(|| {
            data.get("timeline")
                .and_then(|t| t.get("instructions"))
                .and_then(|i| i.as_array())
        })
        .or_else(|| data.get("instructions").and_then(|i| i.as_array()));

    let Some(instructions) = instructions else {
        return out;
    };

    // Kumpulkan simpul `TimelineTrend` dari kedalaman mana pun; X menaruhnya di
    // `content.items[].item.itemContent` dan kadang langsung di `content`.
    fn collect(node: &serde_json::Value, raw: &mut Vec<serde_json::Value>) {
        match node {
            serde_json::Value::Object(map) => {
                if map.get("__typename").and_then(|t| t.as_str()) == Some("TimelineTrend") {
                    raw.push(node.clone());
                    return;
                }
                for v in map.values() {
                    collect(v, raw);
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr {
                    collect(v, raw);
                }
            }
            _ => {}
        }
    }

    let mut raw_nodes = Vec::new();
    for inst in instructions {
        collect(inst, &mut raw_nodes);
    }

    for node in &raw_nodes {
        let Some(name) = node.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        if !seen.insert(name.to_string()) {
            continue;
        }

        // Konteks: "Trending now · Sports · 54 posts".
        let social = node
            .get("social_context")
            .and_then(|c| c.get("text"))
            .and_then(|t| t.as_str());

        out.push(Trend {
            name: name.to_string(),
            rank: node.get("rank").and_then(to_u32),
            context: social.map(str::to_owned).or_else(|| {
                first_str(
                    node,
                    &[&["trend_metadata", "domain_context"], &["domain_context"]],
                )
                .map(str::to_owned)
            }),
            meta_description: node
                .get("trend_metadata")
                .and_then(|m| m.get("meta_description"))
                .and_then(|m| m.as_str())
                .map(str::to_owned),
            // `twitter://trending/<id>` bukan URL web; ubah jadi tautan pencarian
            // X yang bisa dibuka manusia/agent.
            url: trend_web_url(node, name),
        });
    }

    out
}

/// Ubah tautan deep-link trend menjadi URL web yang dapat dibuka.
///
/// X mengirim `twitter://trending/<id>`; yang berguna adalah halaman pencarian
/// untuk nama trend tersebut.
fn trend_web_url(node: &serde_json::Value, name: &str) -> Option<String> {
    if let Some(url) = first_str(node, &[&["trend_url", "url"], &["url", "url"]]) {
        if url.starts_with("http://") || url.starts_with("https://") {
            return Some(url.to_owned());
        }
        if let Some(id) = url.strip_prefix("twitter://trending/") {
            return Some(format!("https://x.com/i/trending/{id}"));
        }
    }
    // Fallback: pencarian nama trend.
    Some(format!(
        "https://x.com/search?q={}&src=trend_click",
        urlencode_query(name)
    ))
}

/// Persen-encode untuk query string (spasi jadi `%20`).
fn urlencode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn decode_created_id(data: &serde_json::Value) -> Option<TweetId> {
    data.get("create_tweet")
        .and_then(|ct| ct.get("tweet_results"))
        .and_then(|tr| tr.get("result"))
        .and_then(|res| res.get("rest_id"))
        .and_then(|id| id.as_str())
        .map(|s| TweetId(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tanggal_x_valid() {
        let raw = "Wed Oct 10 20:19:24 +0000 2018";
        let dt = parse_x_date(raw);
        assert_eq!(dt.year(), 2018);
        assert_eq!(dt.month() as u8, 10);
        assert_eq!(dt.day(), 10);
        assert_eq!(dt.hour(), 20);
        assert_eq!(dt.minute(), 19);
        assert_eq!(dt.second(), 24);
    }

    #[test]
    fn decode_tweet_dengan_visibility_results_dan_note_tweet() {
        let fixture = serde_json::json!({
            "__typename": "TweetWithVisibilityResults",
            "tweet": {
                "rest_id": "1840000000000000000",
                "core": {
                    "user_results": {
                        "result": {
                            "legacy": {
                                "screen_name": "penulis",
                                "name": "Penulis Hebat",
                                "verified": true
                            }
                        }
                    }
                },
                "legacy": {
                    "full_text": "teks pendek terpotong...",
                    "created_at": "Mon Sep 29 12:00:00 +0000 2026",
                    "favorite_count": 42,
                    "retweet_count": 10,
                    "reply_count": 5,
                    "bookmark_count": 2,
                    "extended_entities": {
                        "media": [
                            {
                                "type": "video",
                                "media_url_https": "https://pbs.twimg.com/thumb.jpg",
                                "ext_alt_text": "video pemandangan",
                                "video_info": {
                                    "variants": [
                                        {"content_type": "video/mp4", "bitrate": 320000, "url": "https://video.twimg.com/low.mp4"},
                                        {"content_type": "video/mp4", "bitrate": 832000, "url": "https://video.twimg.com/high.mp4"},
                                        {"content_type": "application/x-mpegURL", "url": "https://video.twimg.com/playlist.m3u8"}
                                    ]
                                }
                            }
                        ]
                    }
                },
                "note_tweet": {
                    "note_tweet_results": {
                        "result": {
                            "text": "ini teks lengkap dari note tweet yang sangat panjang melebihi batas 280 karakter"
                        }
                    }
                },
                "views": {
                    "count": "1500"
                }
            }
        });

        let tweet = decode_tweet(&fixture).expect("harus berhasil didecode");
        assert_eq!(tweet.id.0, "1840000000000000000");
        assert_eq!(tweet.author.handle.0, "penulis");
        assert_eq!(tweet.author.display_name, "Penulis Hebat");
        assert!(tweet.author.verified);
        assert_eq!(
            tweet.text,
            "ini teks lengkap dari note tweet yang sangat panjang melebihi batas 280 karakter"
        );
        assert_eq!(
            tweet.url,
            "https://x.com/penulis/status/1840000000000000000"
        );

        // Metrics
        let m = tweet.metrics.expect("metrics harus ada");
        assert_eq!(m.likes, Some(42));
        assert_eq!(m.reposts, Some(10));
        assert_eq!(m.replies, Some(5));
        assert_eq!(m.bookmarks, Some(2));
        assert_eq!(m.views, Some(1500));

        // Media
        assert_eq!(tweet.media.len(), 1);
        let media = &tweet.media[0];
        assert_eq!(media.kind, MediaKind::Video);
        assert_eq!(
            media.url, "https://video.twimg.com/high.mp4",
            "harus memilih bitrate tertinggi"
        );
        assert_eq!(
            media.thumbnail_url.as_deref(),
            Some("https://pbs.twimg.com/thumb.jpg")
        );
        assert_eq!(media.alt.as_deref(), Some("video pemandangan"));
    }

    #[test]
    fn decode_user_sukses() {
        let fixture = serde_json::json!({
            "rest_id": "44196397",
            "is_blue_verified": true,
            "legacy": {
                "screen_name": "elonmusk",
                "name": "Elon Musk",
                "description": "Tech & AI",
                "followers_count": 200000000,
                "verified": false
            }
        });

        let user = decode_user(&fixture).expect("user harus berhasil");
        assert_eq!(user.id, "44196397");
        assert_eq!(user.handle.0, "elonmusk");
        assert_eq!(user.display_name, "Elon Musk");
        assert!(
            user.verified,
            "is_blue_verified harus membuat verified true"
        );
        assert_eq!(user.bio.as_deref(), Some("Tech & AI"));
        assert_eq!(user.followers, Some(200000000));
    }

    /// Bentuk yang dikirim X saat ini (terverifikasi 2026-09-30): identitas ada
    /// di `core`, bio di `profile_bio`, followers di `relationship_counts`.
    /// Uji ini mengunci kedua bentuk agar perubahan X tidak lolos diam-diam.
    #[test]
    fn decode_user_bentuk_baru_core_profile_bio() {
        let fixture = serde_json::json!({
            "user": {
                "result": {
                    "__typename": "User",
                    "rest_id": "1893165876289089536",
                    "is_blue_verified": false,
                    "core": {
                        "created_at": "Sat Feb 22 05:08:43 +0000 2025",
                        "name": "Rian mot",
                        "screen_name": "study47490"
                    },
                    "profile_bio": { "description": "", "entities": { "description": {} } },
                    "relationship_counts": { "followers": 0, "following": 45 },
                    "verification": { "verified": false }
                }
            }
        });

        let user = decode_user(&fixture).expect("bentuk baru harus terdekode");
        assert_eq!(user.id, "1893165876289089536");
        assert_eq!(user.handle.0, "study47490");
        assert_eq!(user.display_name, "Rian mot");
        assert!(!user.verified);
        assert_eq!(user.bio, None, "bio kosong tidak dianggap bio");
        assert_eq!(user.followers, Some(0));
    }

    /// `verification.verified` pada bentuk baru harus dihormati.
    #[test]
    fn decode_user_verification_bentuk_baru() {
        let fixture = serde_json::json!({
            "result": {
                "rest_id": "1",
                "core": { "screen_name": "terverifikasi", "name": "Terverifikasi" },
                "verification": { "verified": true },
                "relationship_counts": { "followers": 1234 }
            }
        });
        let user = decode_user(&fixture).unwrap();
        assert!(user.verified);
        assert_eq!(user.followers, Some(1234));
    }

    /// Bentuk nyata dari X (2026-09-30): trend ada di
    /// `content.items[].item.itemContent`, bukan `content.itemContent`.
    #[test]
    fn decode_trends_bentuk_items_timeline_trend() {
        let fixture = serde_json::json!({
            "timeline": {
                "timeline": {
                    "instructions": [
                        {
                            "type": "TimelineAddEntries",
                            "entries": [
                                {
                                    "entryId": "stories-1",
                                    "content": {
                                        "entryType": "TimelineTimelineModule",
                                        "items": [
                                            {
                                                "entryId": "stories-1-trend-9",
                                                "item": {
                                                    "itemContent": {
                                                        "__typename": "TimelineTrend",
                                                        "itemType": "TimelineTrend",
                                                        "name": "Aston Martin Locks in Alonso",
                                                        "social_context": {
                                                            "contextType": "Facepile",
                                                            "text": "Trending now · Sports · 54 posts"
                                                        },
                                                        "trend_metadata": {
                                                            "url": { "url": "twitter://trending/2105201760428044353", "urlType": "DeepLink" }
                                                        },
                                                        "trend_url": { "url": "twitter://trending/2105201760428044353", "urlType": "DeepLink" }
                                                    }
                                                }
                                            }
                                        ]
                                    }
                                },
                                {
                                    "entryId": "frame-1",
                                    "content": {
                                        "itemContent": { "__typename": "TimelineFrame", "itemType": "TimelineFrame" }
                                    }
                                }
                            ]
                        }
                    ]
                }
            }
        });

        let trends = decode_trends(&fixture);
        assert_eq!(trends.len(), 1, "hanya TimelineTrend yang diambil");
        let t = &trends[0];
        assert_eq!(t.name, "Aston Martin Locks in Alonso");
        assert_eq!(
            t.context.as_deref(),
            Some("Trending now · Sports · 54 posts")
        );
        assert_eq!(t.rank, None, "X tidak mengirim rank");
        assert_eq!(
            t.url.as_deref(),
            Some("https://x.com/i/trending/2105201760428044353"),
            "deep-link harus jadi URL web"
        );
    }

    /// Bila X memakai tautan http, URL dipakai apa adanya.
    #[test]
    fn decode_trends_memakai_url_http_bila_ada() {
        let fixture = serde_json::json!({
            "instructions": [{
                "entries": [{
                    "content": { "items": [{ "item": { "itemContent": {
                        "__typename": "TimelineTrend",
                        "name": "#RustLang",
                        "rank": "3",
                        "trend_url": { "url": "https://x.com/hashtag/RustLang" }
                    }}}]}
                }]
            }]
        });
        let trends = decode_trends(&fixture);
        assert_eq!(
            trends[0].url.as_deref(),
            Some("https://x.com/hashtag/RustLang")
        );
        assert_eq!(trends[0].rank, Some(3), "rank string harus dibaca");
    }

    #[test]
    fn decode_trends_membuang_entri_tanpa_nama_dan_duplikat() {
        let fixture = serde_json::json!({
            "instructions": [{
                "entries": [{
                    "content": { "items": [
                        { "item": { "itemContent": { "__typename": "TimelineTrend", "rank": 1 } } },
                        { "item": { "itemContent": { "__typename": "TimelineTrend", "name": "#A" } } },
                        { "item": { "itemContent": { "__typename": "TimelineTrend", "name": "#A" } } }
                    ]}
                }]
            }]
        });
        let trends = decode_trends(&fixture);
        assert_eq!(trends.len(), 1, "tanpa nama dibuang, duplikat disatukan");
        assert_eq!(trends[0].name, "#A");
    }

    /// Regresi: X memakai `timeline.timeline.instructions` untuk timeline user;
    /// dulu hanya `timeline_v2` yang dibaca sehingga hasilnya selalu kosong.
    #[test]
    fn instructions_of_menerima_semua_sarang_timeline() {
        let nested = serde_json::json!({ "timeline": { "timeline": { "instructions": [1, 2] } } });
        assert_eq!(instructions_of(&nested).as_array().unwrap().len(), 2);

        let single = serde_json::json!({ "timeline": { "instructions": [3] } });
        assert_eq!(instructions_of(&single).as_array().unwrap().len(), 1);

        let direct = serde_json::json!({ "instructions": [4, 5, 6] });
        assert_eq!(instructions_of(&direct).as_array().unwrap().len(), 3);

        // Tidak memuat instructions: kembalikan simpul apa adanya (bukan panic).
        let none = serde_json::json!({ "lain": 1 });
        assert_eq!(instructions_of(&none), &none);
    }

    #[test]
    fn decode_trends_bentuk_tak_dikenal_menghasilkan_kosong() {
        assert!(decode_trends(&serde_json::json!({})).is_empty());
        assert!(decode_trends(&serde_json::json!({ "timeline": {} })).is_empty());
        // Timeline berisi tweet (bukan trend) tidak menghasilkan trend.
        let tweets_only = serde_json::json!({
            "timeline": { "timeline": { "instructions": [{ "entries": [
                { "content": { "itemContent": { "__typename": "TimelineTweet" } } }
            ]}]}}
        });
        assert!(decode_trends(&tweets_only).is_empty());
    }

    #[test]
    fn decode_created_id_sukses() {
        let fixture = serde_json::json!({
            "create_tweet": {
                "tweet_results": {
                    "result": {
                        "rest_id": "1840123456789012345"
                    }
                }
            }
        });

        let id = decode_created_id(&fixture).expect("harus temukan rest_id");
        assert_eq!(id.0, "1840123456789012345");
    }

    #[test]
    fn decode_entries_timeline_dan_cursor() {
        let fixture = serde_json::json!({
            "instructions": [
                {
                    "type": "TimelineAddEntries",
                    "entries": [
                        {
                            "entryId": "tweet-1",
                            "content": {
                                "itemContent": {
                                    "tweet_results": {
                                        "result": {
                                            "rest_id": "111",
                                            "legacy": {
                                                "full_text": "tweet pertama",
                                                "created_at": "Mon Sep 29 12:00:00 +0000 2026"
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        {
                            "entryId": "cursor-bottom-12345",
                            "content": {
                                "value": "cursor_next_token_xyz"
                            }
                        }
                    ]
                }
            ]
        });

        let (tweets, cursor) = decode_entries(&fixture);
        assert_eq!(tweets.len(), 1);
        assert_eq!(tweets[0].id.0, "111");
        assert_eq!(cursor.as_deref(), Some("cursor_next_token_xyz"));
    }
}
