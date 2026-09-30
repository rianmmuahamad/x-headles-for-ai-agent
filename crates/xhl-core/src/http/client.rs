use reqwest::header::{HeaderMap as ReqwestHeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;
use std::future::Future;
use std::time::Duration;

use super::headers::HttpProfile;
use crate::error::XhlError;

/// Bagian data multipart untuk upload.
pub struct MultipartPart {
    pub name: &'static str,
    pub filename: Option<String>,
    pub content_type: Option<&'static str>,
    pub bytes: Vec<u8>,
}

/// Respons yang sudah dinormalisasi: tidak ada tipe transport pihak ketiga yang bocor.
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name_lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == name_lower)
            .map(|(_, v)| v.as_str())
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json<T: DeserializeOwned>(&self) -> Result<T, XhlError> {
        serde_json::from_str(&self.body).map_err(|e| {
            XhlError::Internal(format!(
                "gagal membaca respons JSON: {e}; body: {}",
                self.body
            ))
        })
    }
}

pub type HeaderMap = Vec<(String, String)>;

/// Klien HTTP. Dibagi lewat `clone`: handle di dalamnya (pool koneksi) tetap satu.
#[derive(Clone)]
pub struct Client {
    inner: InnerClient,
}

/// Tanpa fitur `impersonate`: `reqwest` biasa (Rustls).
#[cfg(not(feature = "impersonate"))]
type InnerClient = reqwest::Client;
#[cfg(not(feature = "impersonate"))]
type ErrType = reqwest::Error;
/// Dengan fitur `impersonate`: `wreq`, yang menyamakan fingerprint TLS/JA3.
#[cfg(feature = "impersonate")]
type InnerClient = wreq::Client;
#[cfg(feature = "impersonate")]
type ErrType = wreq::Error;

#[cfg(not(feature = "impersonate"))]
type MultipartForm = reqwest::multipart::Form;
#[cfg(not(feature = "impersonate"))]
type MultipartPartBuilder = reqwest::multipart::Part;
#[cfg(feature = "impersonate")]
type MultipartForm = wreq::multipart::Form;
#[cfg(feature = "impersonate")]
type MultipartPartBuilder = wreq::multipart::Part;

/// Percobaan ulang untuk kegagalan koneksi sesaat.
///
/// Cloudflare di depan X menolak koneksi secara sporadis (kadang `000` tanpa
/// balasan HTTP). Percobaan berikutnya biasanya berhasil dalam hitungan detik.
/// Nilai dapat diatur lewat `XHL_CONFIG` (`retry_attempts`, `retry_backoff_ms`,
/// `request_timeout_secs`); default di sini sama dengan konstanta yang dulu.
const CONNECT_ATTEMPTS_DEFAULT: usize = crate::native::config::defaults::RETRY_ATTEMPTS;
const CONNECT_BACKOFF_MS_DEFAULT: u64 = crate::native::config::defaults::RETRY_BACKOFF_MS;

fn connect_attempts() -> usize {
    crate::native::config_usize("retry_attempts", CONNECT_ATTEMPTS_DEFAULT)
}

fn connect_backoff_ms() -> u64 {
    crate::native::config_u64("retry_backoff_ms", CONNECT_BACKOFF_MS_DEFAULT)
}

fn request_timeout_secs() -> u64 {
    crate::native::config_u64(
        "request_timeout_secs",
        crate::native::config::defaults::REQUEST_TIMEOUT_SECS,
    )
}

impl Client {
    /// Jalankan operasi kirim, ulangi hanya bila gagal **sebelum** respons diterima.
    async fn with_retry<F, Fut>(stage: &'static str, op: F) -> Result<HttpResponse, XhlError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<HttpResponse, XhlError>>,
    {
        let mut last: Option<XhlError> = None;
        for attempt in 0..connect_attempts() {
            match op().await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    // Hanya kegagalan transport yang boleh diulang; error HTTP
                    // sudah dinormalisasi menjadi respons.
                    let retryable = matches!(e, XhlError::Network(_) | XhlError::Timeout { .. });
                    if !retryable || attempt + 1 == connect_attempts() {
                        return Err(e);
                    }
                    let wait = connect_backoff_ms() * (attempt as u64 + 1);
                    tracing::debug!(stage, attempt = attempt + 1, wait_ms = wait, error = %e, "koneksi gagal, mencoba lagi");
                    tokio::time::sleep(Duration::from_millis(wait)).await;
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| XhlError::Internal(format!("{stage}: percobaan habis"))))
    }

    pub fn new(profile: &HttpProfile) -> Result<Self, XhlError> {
        Ok(Self {
            inner: Self::build_inner(profile)?,
        })
    }

    /// Bangun client. Profil TLS dan `user-agent` harus berasal dari deskripsi
    /// yang sama; karena itu `profile` ikut menentukan emulasi.
    #[cfg(not(feature = "impersonate"))]
    fn build_inner(_profile: &HttpProfile) -> Result<InnerClient, XhlError> {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(request_timeout_secs()))
            .gzip(true)
            .build()
            .map_err(|e| XhlError::Internal(format!("gagal membuat HTTP client: {e}")))
    }

    #[cfg(feature = "impersonate")]
    fn build_inner(profile: &HttpProfile) -> Result<InnerClient, XhlError> {
        let emulation = match profile.name {
            "firefox_133" => wreq_util::Emulation::Firefox133,
            "chrome_131" => wreq_util::Emulation::Chrome131,
            other => {
                return Err(XhlError::Config(format!(
                    "profil '{other}' tidak punya padanan emulasi TLS; \
                     pakai firefox_133 atau chrome_131"
                )))
            }
        };

        wreq::Client::builder()
            .emulation(emulation)
            .timeout(Duration::from_secs(request_timeout_secs()))
            .gzip(true)
            .build()
            .map_err(|e| {
                XhlError::Internal(format!("gagal membuat HTTP client (impersonate): {e}"))
            })
    }

    fn to_reqwest_headers(headers: &HeaderMap) -> Result<ReqwestHeaderMap, XhlError> {
        let mut map = ReqwestHeaderMap::new();
        for (k, v) in headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| XhlError::Internal(format!("invalid header name '{k}': {e}")))?;
            let val = HeaderValue::from_str(v)
                .map_err(|e| XhlError::Internal(format!("invalid header value for '{k}': {e}")))?;
            map.insert(name, val);
        }
        Ok(map)
    }

    #[cfg(not(feature = "impersonate"))]
    async fn handle_response(resp: reqwest::Response) -> Result<HttpResponse, XhlError> {
        let status = resp.status().as_u16();
        let headers = Self::dump_headers(resp.headers());
        let body = resp
            .text()
            .await
            .map_err(|e| Self::map_err(e, "membaca body"))?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    #[cfg(feature = "impersonate")]
    async fn handle_response(resp: wreq::Response) -> Result<HttpResponse, XhlError> {
        let status = resp.status().as_u16();
        let headers = Self::dump_headers(resp.headers());
        let body = resp
            .text()
            .await
            .map_err(|e| Self::map_err(e, "membaca body"))?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    /// Salin header ke bentuk normal (nama huruf kecil).
    fn dump_headers(headers: &ReqwestHeaderMap) -> Vec<(String, String)> {
        headers
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|val| (k.as_str().to_ascii_lowercase(), val.to_string()))
            })
            .collect()
    }

    fn map_err(e: ErrType, stage: &'static str) -> XhlError {
        if e.is_timeout() {
            XhlError::Timeout { stage }
        } else if e.is_connect() || e.is_request() {
            XhlError::Network(e.to_string())
        } else {
            XhlError::Internal(format!("{stage}: {e}"))
        }
    }

    pub async fn get(&self, url: &str, headers: &HeaderMap) -> Result<HttpResponse, XhlError> {
        let r_headers = Self::to_reqwest_headers(headers)?;
        Self::with_retry("HTTP GET", || async {
            let resp = self
                .inner
                .get(url)
                .headers(r_headers.clone())
                .send()
                .await
                .map_err(|e| Self::map_err(e, "HTTP GET"))?;
            Self::handle_response(resp).await
        })
        .await
    }

    pub async fn post_json(
        &self,
        url: &str,
        headers: &HeaderMap,
        body: &serde_json::Value,
    ) -> Result<HttpResponse, XhlError> {
        let r_headers = Self::to_reqwest_headers(headers)?;
        Self::with_retry("HTTP POST JSON", || async {
            let resp = self
                .inner
                .post(url)
                .headers(r_headers.clone())
                .json(body)
                .send()
                .await
                .map_err(|e| Self::map_err(e, "HTTP POST JSON"))?;
            Self::handle_response(resp).await
        })
        .await
    }

    pub async fn post_multipart(
        &self,
        url: &str,
        headers: &HeaderMap,
        parts: Vec<MultipartPart>,
    ) -> Result<HttpResponse, XhlError> {
        let r_headers = Self::to_reqwest_headers(headers)?;

        // `Form` tidak bisa dikloning, jadi form dibangun ulang tiap percobaan.
        // Bagian multipart memuat byte berkas, karena itu `MultipartPart`
        // dijadikan `Arc` agar percobaan ulang tidak menyalin isi berkas.
        let parts: Vec<std::sync::Arc<MultipartPart>> =
            parts.into_iter().map(std::sync::Arc::new).collect();

        Self::with_retry("HTTP POST multipart", || async {
            let mut form = MultipartForm::new();
            for p in &parts {
                let mut part = MultipartPartBuilder::bytes(p.bytes.clone());
                if let Some(mime) = p.content_type {
                    part = part
                        .mime_str(mime)
                        .map_err(|e| XhlError::Internal(format!("invalid mime '{mime}': {e}")))?;
                }
                if let Some(fn_str) = &p.filename {
                    part = part.file_name(fn_str.clone());
                }
                form = form.part(p.name, part);
            }

            let resp = self
                .inner
                .post(url)
                .headers(r_headers.clone())
                .multipart(form)
                .send()
                .await
                .map_err(|e| Self::map_err(e, "HTTP POST multipart"))?;
            Self::handle_response(resp).await
        })
        .await
    }
}
