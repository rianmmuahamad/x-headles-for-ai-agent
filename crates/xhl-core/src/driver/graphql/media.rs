use crate::error::XhlError;
use crate::http::client::{Client as HttpClient, MultipartPart};
use crate::http::headers::{self, HttpProfile, RequestHeaders};
use crate::session::Cookies;
use std::path::Path;
use std::time::Duration;

const UPLOAD_URL: &str = "https://upload.twitter.com/1.1/media/upload.json";
const METADATA_URL: &str = "https://upload.twitter.com/1.1/media/metadata/create.json";
/// Ukuran potongan APPEND default (5 MiB). Dapat diatur `XHL_CONFIG`:
/// `media_chunk_bytes`. Potongan lebih kecil memperhalus progres tapi menambah
/// round-trip; jangan melebihi batas X (5 MB) — nilai config dipakai apa adanya.
const CHUNK_SIZE_DEFAULT: usize = crate::native::config::defaults::MEDIA_CHUNK_BYTES;

fn chunk_size() -> usize {
    crate::native::config_usize("media_chunk_bytes", CHUNK_SIZE_DEFAULT).max(1)
}

pub async fn upload(
    client: &HttpClient,
    cookies: &Cookies,
    profile: &'static HttpProfile,
    path: &Path,
    alt: Option<&str>,
) -> Result<String, XhlError> {
    let bytes = std::fs::read(path).map_err(|e| {
        XhlError::Invalid(format!("tidak bisa membaca media {}: {e}", path.display()))
    })?;

    let is_video = matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("mp4" | "mov" | "webm" | "m4v")
    );

    let media_id = if is_video {
        let media_type = match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("mp4") => "video/mp4",
            Some("mov") => "video/quicktime",
            Some("webm") => "video/webm",
            _ => "video/mp4",
        };
        upload_video_chunked(client, cookies, profile, &bytes, media_type).await?
    } else {
        let content_type = match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => "image/png",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => "image/jpeg",
        };
        let file_name = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("image.jpg")
            .to_string();
        upload_image_simple(client, cookies, profile, bytes, &file_name, content_type).await?
    };

    if let Some(alt_text) = alt {
        let _ = set_alt_text(client, cookies, profile, &media_id, alt_text).await;
    }

    Ok(media_id)
}

async fn upload_image_simple(
    client: &HttpClient,
    cookies: &Cookies,
    profile: &'static HttpProfile,
    bytes: Vec<u8>,
    file_name: &str,
    content_type: &'static str,
) -> Result<String, XhlError> {
    let headers = headers::build(RequestHeaders {
        cookies,
        profile,
        transaction_id: None,
        json_body: false,
    });

    let parts = vec![MultipartPart {
        name: "media",
        filename: Some(file_name.to_string()),
        content_type: Some(content_type),
        bytes,
    }];

    let resp = client.post_multipart(UPLOAD_URL, &headers, parts).await?;
    if !resp.is_success() {
        return Err(XhlError::Internal(format!(
            "upload image gagal (HTTP {}): {}",
            resp.status, resp.body
        )));
    }

    let val: serde_json::Value = resp.json()?;
    let media_id = val
        .get("media_id_string")
        .and_then(|v| v.as_str())
        .or_else(|| val.get("media_id").and_then(|v| v.as_str()))
        .ok_or_else(|| {
            XhlError::Internal(format!(
                "respons upload tidak memiliki media_id_string: {}",
                resp.body
            ))
        })?;

    Ok(media_id.to_string())
}

async fn upload_video_chunked(
    client: &HttpClient,
    cookies: &Cookies,
    profile: &'static HttpProfile,
    bytes: &[u8],
    media_type: &'static str,
) -> Result<String, XhlError> {
    let total_bytes = bytes.len();

    // 1. INIT
    let headers = headers::build(RequestHeaders {
        cookies,
        profile,
        transaction_id: None,
        json_body: false,
    });

    let init_parts = vec![
        MultipartPart {
            name: "command",
            filename: None,
            content_type: None,
            bytes: b"INIT".to_vec(),
        },
        MultipartPart {
            name: "total_bytes",
            filename: None,
            content_type: None,
            bytes: total_bytes.to_string().into_bytes(),
        },
        MultipartPart {
            name: "media_type",
            filename: None,
            content_type: None,
            bytes: media_type.as_bytes().to_vec(),
        },
    ];

    let resp = client
        .post_multipart(UPLOAD_URL, &headers, init_parts)
        .await?;
    if !resp.is_success() {
        return Err(XhlError::Internal(format!(
            "video INIT gagal (HTTP {}): {}",
            resp.status, resp.body
        )));
    }
    let val: serde_json::Value = resp.json()?;
    let media_id = val
        .get("media_id_string")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            XhlError::Internal(format!("video INIT tanpa media_id_string: {}", resp.body))
        })?
        .to_string();

    // 2. APPEND
    for (segment_index, chunk) in bytes.chunks(chunk_size()).enumerate() {
        let headers = headers::build(RequestHeaders {
            cookies,
            profile,
            transaction_id: None,
            json_body: false,
        });

        let append_parts = vec![
            MultipartPart {
                name: "command",
                filename: None,
                content_type: None,
                bytes: b"APPEND".to_vec(),
            },
            MultipartPart {
                name: "media_id",
                filename: None,
                content_type: None,
                bytes: media_id.as_bytes().to_vec(),
            },
            MultipartPart {
                name: "segment_index",
                filename: None,
                content_type: None,
                bytes: segment_index.to_string().into_bytes(),
            },
            MultipartPart {
                name: "media",
                filename: Some("blob".to_string()),
                content_type: Some("application/octet-stream"),
                bytes: chunk.to_vec(),
            },
        ];

        let append_resp = client
            .post_multipart(UPLOAD_URL, &headers, append_parts)
            .await?;
        if !append_resp.is_success() {
            return Err(XhlError::Internal(format!(
                "video APPEND segment {} gagal (HTTP {}): {}",
                segment_index, append_resp.status, append_resp.body
            )));
        }
    }

    // 3. FINALIZE
    let headers = headers::build(RequestHeaders {
        cookies,
        profile,
        transaction_id: None,
        json_body: false,
    });

    let finalize_parts = vec![
        MultipartPart {
            name: "command",
            filename: None,
            content_type: None,
            bytes: b"FINALIZE".to_vec(),
        },
        MultipartPart {
            name: "media_id",
            filename: None,
            content_type: None,
            bytes: media_id.as_bytes().to_vec(),
        },
    ];

    let fin_resp = client
        .post_multipart(UPLOAD_URL, &headers, finalize_parts)
        .await?;
    if !fin_resp.is_success() {
        return Err(XhlError::Internal(format!(
            "video FINALIZE gagal (HTTP {}): {}",
            fin_resp.status, fin_resp.body
        )));
    }

    let fin_val: serde_json::Value = fin_resp.json()?;

    // 4. STATUS check jika ada processing_info
    if let Some(info) = fin_val.get("processing_info") {
        let mut state = info
            .get("state")
            .and_then(|s| s.as_str())
            .unwrap_or("pending")
            .to_string();
        let mut attempts = 0;

        while state == "pending" || state == "in_progress" {
            attempts += 1;
            if attempts > 30 {
                return Err(XhlError::Timeout {
                    stage: "media_processing",
                });
            }
            tokio::time::sleep(Duration::from_secs(2)).await;

            let check_url = format!("{UPLOAD_URL}?command=STATUS&media_id={media_id}");
            let check_headers = headers::build(RequestHeaders {
                cookies,
                profile,
                transaction_id: None,
                json_body: false,
            });
            let check_resp = client.get(&check_url, &check_headers).await?;
            if !check_resp.is_success() {
                return Err(XhlError::Internal(format!(
                    "check STATUS video gagal: {}",
                    check_resp.body
                )));
            }
            let check_val: serde_json::Value = check_resp.json()?;
            if let Some(p) = check_val.get("processing_info") {
                state = p
                    .get("state")
                    .and_then(|s| s.as_str())
                    .unwrap_or("pending")
                    .to_string();
                if state == "failed" {
                    let msg = p
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("video processing failed");
                    return Err(XhlError::Internal(msg.to_string()));
                }
            } else {
                break;
            }
        }
    }

    Ok(media_id)
}

async fn set_alt_text(
    client: &HttpClient,
    cookies: &Cookies,
    profile: &'static HttpProfile,
    media_id: &str,
    alt_text: &str,
) -> Result<(), XhlError> {
    let headers = headers::build(RequestHeaders {
        cookies,
        profile,
        transaction_id: None,
        json_body: true,
    });
    let body = serde_json::json!({
        "media_id": media_id,
        "alt_text": {
            "text": alt_text
        }
    });

    let resp = client.post_json(METADATA_URL, &headers, &body).await?;
    if !resp.is_success() {
        tracing::warn!(media_id = %media_id, error = %resp.body, "gagal menyetel alt text media");
    }
    Ok(())
}
