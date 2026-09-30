pub mod decode;
pub mod dto;
pub mod features;
pub mod media;

use async_trait::async_trait;
use std::path::Path;

use crate::antibot::TransactionSigner;
use crate::domain::{
    Handle, Metrics, Page, Post, Posted, SearchQuery, SearchRanking, TimelineKind, Trend,
    TrendCategory, Tweet, TweetId, UserProfile,
};
use crate::driver::{XReader, XWriter};
use crate::error::{ForbiddenReason, XhlError};
use crate::http::client::{Client as HttpClient, HttpResponse};
use crate::http::headers::{self, HttpProfile, RequestHeaders};
use crate::limiter::RateLimiter;
use crate::query::QueryRegistry;
use crate::session::http_status::Status;
use crate::session::{Cookies, SessionStatus};
use crate::store::StoreHandle;

/// Operasi yang wajib dapat di-resolve registry.
///
/// Nama mengikuti bundle X saat ini (terverifikasi 2026-09-30): timeline home
/// memakai `TVHomeMixer`, bukan `HomeTimeline`/`HomeLatestTimeline` yang sudah
/// tidak dikirim.
pub const CORE_OPERATIONS: &[&str] = &[
    "SearchTimeline",
    "TweetDetail",
    "UserByScreenName",
    "UserTweets",
    "TVHomeMixer",
    "GenericTimelineById",
    "CreateTweet",
];

/// Operasi timeline home untuk tiap jenis yang didukung.
const OP_HOME: &str = "TVHomeMixer";

/// Timeline id untuk tiap kategori trend.
///
/// Konstanta milik X (base64), bukan hash `queryId` — tidak berotasi bersama
/// bundle. Disalin dari implementasi rujukan yang berjalan; bila X mengubahnya,
/// `xhl trends` mengembalikan daftar kosong (bukan galat), dan nilainya dapat
/// diperbarui di sini.
fn trend_timeline_id(category: TrendCategory) -> &'static str {
    match category {
        TrendCategory::Trending => "VGltZWxpbmU6DAC2CwABAAAACHRyZW5kaW5nAAA",
        TrendCategory::News => "VGltZWxpbmU6DAC2CwABAAAABG5ld3MAAA",
        TrendCategory::Sport => "VGltZWxpbmU6DAC2CwABAAAABnNwb3J0cwAA",
        TrendCategory::Entertainment => "VGltZWxpbmU6DAC2CwABAAAADWVudGVydGFpbm1lbnQAAA",
    }
}

pub struct Diagnostics {
    pub signer_ready: bool,
    pub known_operations: Vec<(String, String)>,
    pub missing_operations: Vec<&'static str>,
}

pub struct GraphQlConfig {
    pub client: HttpClient,
    pub registry: QueryRegistry,
    pub signer: TransactionSigner,
    pub limiter: RateLimiter,
    pub cookies: Cookies,
    pub profile: &'static HttpProfile,
    pub account: String,
    pub store: Option<StoreHandle>,
    pub no_wait: bool,
}

pub struct GraphQlDriver {
    pub client: HttpClient,
    pub queries: QueryRegistry,
    pub signer: TransactionSigner,
    pub limiter: RateLimiter,
    pub cookies: Cookies,
    pub profile: &'static HttpProfile,
    pub account: String,
    pub store: Option<StoreHandle>,
    pub no_wait: bool,
}

impl GraphQlDriver {
    pub fn new(cfg: GraphQlConfig) -> Self {
        Self {
            client: cfg.client,
            queries: cfg.registry,
            signer: cfg.signer,
            limiter: cfg.limiter,
            cookies: cfg.cookies,
            profile: cfg.profile,
            account: cfg.account,
            store: cfg.store,
            no_wait: cfg.no_wait,
        }
    }

    pub async fn diagnostics(&self) -> Diagnostics {
        let known = self.queries.known().await;
        let known_names: std::collections::HashSet<_> =
            known.iter().map(|(k, _)| k.as_str()).collect();
        let missing: Vec<&'static str> = CORE_OPERATIONS
            .iter()
            .copied()
            .filter(|op| !known_names.contains(*op))
            .collect();

        Diagnostics {
            signer_ready: self.signer.is_ready(),
            known_operations: known,
            missing_operations: missing,
        }
    }

    pub async fn rest_get(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<HttpResponse, XhlError> {
        let mut url = format!("https://x.com/i/api{path}");
        if !query.is_empty() {
            let encoded: Vec<String> = query
                .iter()
                .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
                .collect();
            url.push('?');
            url.push_str(&encoded.join("&"));
        }

        let headers = headers::build(RequestHeaders {
            cookies: &self.cookies,
            profile: self.profile,
            transaction_id: None,
            json_body: false,
        });

        self.client.get(&url, &headers).await
    }

    pub async fn gql_get(
        &self,
        op: &str,
        variables: serde_json::Value,
        features: serde_json::Value,
    ) -> Result<serde_json::Value, XhlError> {
        self.limiter.acquire(op, self.no_wait).await?;

        // Resolve query ID
        let qid = self.queries.resolve(op).await?;
        let path = format!("/i/api/graphql/{}/{}", qid.as_str(), op);

        // Sign
        let sig = self.signer.sign("GET", &path).await?;

        // Build URL
        let mut url = format!("https://x.com{path}?");
        let vars_str = serde_json::to_string(&variables)
            .map_err(|e| XhlError::Internal(format!("serialize vars: {e}")))?;
        let feats_str = serde_json::to_string(&features)
            .map_err(|e| XhlError::Internal(format!("serialize features: {e}")))?;

        url.push_str(&format!(
            "variables={}&features={}",
            url_encode(&vars_str),
            url_encode(&feats_str)
        ));

        let headers = headers::build(RequestHeaders {
            cookies: &self.cookies,
            profile: self.profile,
            transaction_id: Some(&sig),
            json_body: false,
        });

        let resp = self.client.get(&url, &headers).await?;
        self.limiter.observe(op, &resp.headers);

        // Retry logika jika 404 (query ID basi) atau 403 (transaction ID basi)
        if resp.status == 404 {
            tracing::info!(op = %op, "menerima 404, mencoba refresh queryId...");
            self.queries.refresh().await?;
            let qid_new = self.queries.resolve(op).await?;
            let path_new = format!("/i/api/graphql/{}/{}", qid_new.as_str(), op);
            let sig_new = self.signer.sign("GET", &path_new).await?;
            let url_new = format!(
                "https://x.com{}?variables={}&features={}",
                path_new,
                url_encode(&vars_str),
                url_encode(&feats_str)
            );
            let headers_new = headers::build(RequestHeaders {
                cookies: &self.cookies,
                profile: self.profile,
                transaction_id: Some(&sig_new),
                json_body: false,
            });
            let resp_retry = self.client.get(&url_new, &headers_new).await?;
            self.limiter.observe(op, &resp_retry.headers);
            return dto::unwrap_envelope(resp_retry.status, &resp_retry.body);
        }

        if resp.status == 403 {
            let reason = ForbiddenReason::classify(resp.status, &resp.body);
            if reason == ForbiddenReason::TransactionId {
                tracing::info!(op = %op, "menerima 403 TransactionId, mencoba refresh signer...");
                self.signer.refresh().await?;
                let sig_new = self.signer.sign("GET", &path).await?;
                let headers_new = headers::build(RequestHeaders {
                    cookies: &self.cookies,
                    profile: self.profile,
                    transaction_id: Some(&sig_new),
                    json_body: false,
                });
                let resp_retry = self.client.get(&url, &headers_new).await?;
                self.limiter.observe(op, &resp_retry.headers);
                return dto::unwrap_envelope(resp_retry.status, &resp_retry.body);
            }
        }

        dto::unwrap_envelope(resp.status, &resp.body)
    }

    pub async fn gql_post(
        &self,
        op: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, XhlError> {
        self.limiter.acquire(op, self.no_wait).await?;

        let qid = self.queries.resolve(op).await?;
        let path = format!("/i/api/graphql/{}/{}", qid.as_str(), op);
        let sig = self.signer.sign("POST", &path).await?;

        let url = format!("https://x.com{path}");
        let headers = headers::build(RequestHeaders {
            cookies: &self.cookies,
            profile: self.profile,
            transaction_id: Some(&sig),
            json_body: true,
        });

        let resp = self.client.post_json(&url, &headers, &body).await?;
        self.limiter.observe(op, &resp.headers);

        if resp.status == 404 {
            tracing::info!(op = %op, "POST menerima 404, refresh queryId...");
            self.queries.refresh().await?;
            let qid_new = self.queries.resolve(op).await?;
            let path_new = format!("/i/api/graphql/{}/{}", qid_new.as_str(), op);
            let sig_new = self.signer.sign("POST", &path_new).await?;
            let url_new = format!("https://x.com{path_new}");
            let headers_new = headers::build(RequestHeaders {
                cookies: &self.cookies,
                profile: self.profile,
                transaction_id: Some(&sig_new),
                json_body: true,
            });
            let resp_retry = self.client.post_json(&url_new, &headers_new, &body).await?;
            self.limiter.observe(op, &resp_retry.headers);
            return dto::unwrap_envelope(resp_retry.status, &resp_retry.body);
        }

        if resp.status == 403 {
            let reason = ForbiddenReason::classify(resp.status, &resp.body);
            if reason == ForbiddenReason::TransactionId {
                tracing::info!(op = %op, "POST menerima 403 TransactionId, refresh signer...");
                self.signer.refresh().await?;
                let sig_new = self.signer.sign("POST", &path).await?;
                let headers_new = headers::build(RequestHeaders {
                    cookies: &self.cookies,
                    profile: self.profile,
                    transaction_id: Some(&sig_new),
                    json_body: true,
                });
                let resp_retry = self.client.post_json(&url, &headers_new, &body).await?;
                self.limiter.observe(op, &resp_retry.headers);
                return dto::unwrap_envelope(resp_retry.status, &resp_retry.body);
            }
        }

        dto::unwrap_envelope(resp.status, &resp.body)
    }
}

#[async_trait]
impl XReader for GraphQlDriver {
    async fn health(&self) -> Result<SessionStatus, XhlError> {
        // `/1.1/account/settings.json` sudah tidak ada di X (404 "page does not
        // exist"). `/1.1/account/multi/list.json` mengembalikan identitas akun
        // dan menolak cookie yang tidak berlaku dengan 401/403.
        let resp = self.rest_get("/1.1/account/multi/list.json", &[]).await?;
        if resp.status == 401 || resp.status == 403 {
            return Ok(SessionStatus::Expired {
                detail: format!("HTTP {}", resp.status),
            });
        }
        Ok(SessionStatus::interpret(
            Status::new(resp.status),
            &resp.body,
        ))
    }

    async fn user_by_handle(&self, handle: &Handle) -> Result<UserProfile, XhlError> {
        let vars = serde_json::json!({
            "screen_name": handle.0,
            "withSafetyModeUserFields": true
        });
        let data = self
            .gql_get("UserByScreenName", vars, features::default_features())
            .await?;
        decode::decode_user(&data)
            .ok_or_else(|| XhlError::Internal(format!("gagal mendecode profil @{}", handle.0)))
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<Tweet>, XhlError> {
        // `Top` = tweet paling relevan/berinteraksi; `Latest` = terbaru.
        // `querySource` mengikuti tab yang dipakai X: pencarian biasa memakai
        // `typed_query`, sedangkan masuk dari trend memakai `trend_click`.
        let (product, query_source) = match q.ranking {
            SearchRanking::Latest => ("Latest", "typed_query"),
            SearchRanking::Top => ("Top", "trend_click"),
        };
        let mut vars = serde_json::json!({
            "rawQuery": q.query,
            "count": q.limit,
            "querySource": query_source,
            "product": product
        });
        if let Some(cursor) = &q.cursor {
            vars["cursor"] = serde_json::json!(cursor);
        }

        let data = self
            .gql_get("SearchTimeline", vars, features::default_features())
            .await?;
        let node = data
            .get("search_by_raw_query")
            .and_then(|s| s.get("search_timeline"))
            .and_then(|st| st.get("timeline"))
            .unwrap_or(&serde_json::Value::Null);
        let (tweets, _) = decode::decode_entries(decode::instructions_of(node));
        Ok(tweets)
    }

    async fn timeline(
        &self,
        kind: TimelineKind,
        handle: Option<&Handle>,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<Page<Tweet>, XhlError> {
        let (tweets, next_cursor) = match kind {
            // `TVHomeMixer` melayani dua tab: "For You" dan "Following",
            // dibedakan oleh `timeline_type`.
            TimelineKind::Home | TimelineKind::HomeLatest => {
                let timeline_type = if kind == TimelineKind::HomeLatest {
                    "Following"
                } else {
                    "ForYou"
                };
                let mut vars = serde_json::json!({
                    "timeline_type": timeline_type,
                    "count": limit,
                    "includePromotedContent": false,
                    "withCommunity": true,
                    "withVoice": true,
                });
                if let Some(c) = cursor {
                    vars["cursor"] = serde_json::json!(c);
                }
                let data = self
                    .gql_get(OP_HOME, vars, features::default_features())
                    .await?;
                let node = data.get("home").unwrap_or(&data);
                decode::decode_entries(decode::instructions_of(node))
            }
            TimelineKind::User => {
                let h = handle
                    .ok_or_else(|| XhlError::Invalid("timeline user memerlukan handle".into()))?;
                let user = self.user_by_handle(h).await?;
                let mut vars = serde_json::json!({
                    "userId": user.id,
                    "count": limit,
                    "includePromotedContent": false,
                    "withQuickPromoteEligibilityTweetFields": false,
                    "withVoice": false,
                    "withV2Timeline": true
                });
                if let Some(c) = cursor {
                    vars["cursor"] = serde_json::json!(c);
                }
                let data = self
                    .gql_get("UserTweets", vars, features::default_features())
                    .await?;
                let node = data
                    .get("user")
                    .and_then(|u| u.get("result"))
                    .and_then(|r| r.get("timeline").or_else(|| r.get("timeline_v2")))
                    .unwrap_or(&serde_json::Value::Null);
                decode::decode_entries(decode::instructions_of(node))
            }
        };

        Ok(Page {
            items: tweets,
            next_cursor,
        })
    }

    async fn thread(&self, id: &TweetId) -> Result<Vec<Tweet>, XhlError> {
        let vars = serde_json::json!({
            "focalTweetId": id.0,
            "with_rux_injections": false,
            "includePromotedContent": false,
            "withCommunity": true,
            "withQuickPromoteEligibilityTweetFields": false,
            "withBirdwatchNotes": true,
            "withVoice": false,
            "withV2Timeline": true
        });
        let data = self
            .gql_get("TweetDetail", vars, features::default_features())
            .await?;
        let node = data
            .get("threaded_conversation_with_injections_v2")
            .unwrap_or(&serde_json::Value::Null);
        let (tweets, _) = decode::decode_entries(decode::instructions_of(node));
        Ok(tweets)
    }

    async fn trends(&self, category: TrendCategory, limit: usize) -> Result<Vec<Trend>, XhlError> {
        if limit == 0 {
            return Err(XhlError::Invalid("limit harus > 0".into()));
        }
        let vars = serde_json::json!({
            "timelineId": trend_timeline_id(category),
            "count": limit,
            "withQuickPromoteEligibilityTweetFields": true
        });
        let data = self
            .gql_get("GenericTimelineById", vars, features::default_features())
            .await?;
        Ok(decode::decode_trends(&data))
    }

    async fn tweet_metrics(&self, id: &TweetId) -> Result<Metrics, XhlError> {
        let tweets = self.thread(id).await?;
        for t in tweets {
            if t.id.0 == id.0 {
                return Ok(t.metrics.unwrap_or_default());
            }
        }
        Err(XhlError::Internal(format!(
            "tweet {} tidak ditemukan",
            id.0
        )))
    }

    async fn raw_query(
        &self,
        op: &str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, XhlError> {
        self.gql_get(op, variables, features::default_features())
            .await
    }

    async fn diagnostics(&self) -> Option<Diagnostics> {
        Some(self.diagnostics().await)
    }
}

#[async_trait]
impl XWriter for GraphQlDriver {
    async fn upload_media(&self, path: &Path, alt: Option<&str>) -> Result<String, XhlError> {
        self.limiter.acquire("media_upload", self.no_wait).await?;
        media::upload(&self.client, &self.cookies, self.profile, path, alt).await
    }

    async fn post(&self, post: &Post) -> Result<Posted, XhlError> {
        let mut media_entities = Vec::new();
        for m in &post.media {
            let alt_str = match m {
                crate::domain::MediaInput::Image { alt, .. }
                | crate::domain::MediaInput::Video { alt, .. } => alt.as_deref(),
            };
            let mid = self.upload_media(m.path(), alt_str).await?;
            media_entities.push(serde_json::json!({
                "media_id": mid,
                "tagged_users": []
            }));
        }

        let mut variables = serde_json::json!({
            "tweet_text": post.text,
            "dark_request": false,
            "media": {
                "media_entities": media_entities,
                "possibly_sensitive": false
            },
            "semantic_annotation_ids": []
        });

        if let Some(reply_to) = &post.reply_to {
            variables["reply"] = serde_json::json!({
                "in_reply_to_tweet_id": reply_to.0
            });
        }

        let body = serde_json::json!({
            "variables": variables,
            "features": features::default_features()
        });

        let data = self.gql_post("CreateTweet", body).await?;
        let tweet_id = decode::decode_created_id(&data).ok_or_else(|| {
            let prefix: String = post.text.chars().take(40).collect();
            XhlError::UnknownOutcome {
                hint: format!("verifikasi manual: xhl search \"from:me {prefix}\""),
            }
        })?;

        Ok(Posted {
            id: tweet_id.clone(),
            url: format!("https://x.com/i/status/{}", tweet_id.0),
            posted_at: time::OffsetDateTime::now_utc(),
        })
    }
}

fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}
