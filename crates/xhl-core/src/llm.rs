//! Provider LLM untuk generasi draft.
//!
//! Default `NullProvider` sengaja **selalu gagal** dengan pesan actionable:
//! tidak ada provider terkonfigurasi, dan menebak perilaku akan menyesatkan.

use async_trait::async_trait;

use crate::error::XhlError;
use crate::http::client::Client as HttpClient;
use crate::http::headers::HttpProfile;

/// Opsi generasi.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenOpts {
    pub max_tokens: u32,
    pub temperature: f32,
}

impl Default for GenOpts {
    fn default() -> Self {
        Self {
            max_tokens: 400,
            temperature: 0.7,
        }
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn generate(&self, prompt: &str, opts: GenOpts) -> Result<String, XhlError>;
    fn name(&self) -> &'static str;
}

/// Provider tanpa konfigurasi: setiap panggilan gagal.
pub struct NullProvider;

#[async_trait]
impl LlmProvider for NullProvider {
    async fn generate(&self, _prompt: &str, _opts: GenOpts) -> Result<String, XhlError> {
        Err(XhlError::Config(
            "provider LLM belum dikonfigurasi: set OPENAI_BASE_URL + OPENAI_API_KEY".into(),
        ))
    }

    fn name(&self) -> &'static str {
        "null"
    }
}

/// Provider kompatibel OpenAI (`POST {base}/chat/completions`).
pub struct OpenAiCompatProvider {
    client: HttpClient,
    base_url: String,
    api_key: String,
    model: String,
}

/// Redaksi: kunci API tidak boleh tercetak.
impl std::fmt::Debug for OpenAiCompatProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .finish()
    }
}

impl OpenAiCompatProvider {
    pub const DEFAULT_MODEL: &'static str = "gpt-4o-mini";

    pub fn new(
        client: HttpClient,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// Bangun dari env. `None` bila `OPENAI_BASE_URL`/`OPENAI_API_KEY` tidak lengkap —
    /// itu kondisi normal, bukan kesalahan.
    ///
    /// Kredensial tetap dibaca Rust: keduanya rahasia dan tidak boleh masuk
    /// dump config native. Nama model bukan rahasia, jadi dibaca native
    /// (`XHL_CONFIG` atau `XHL_LLM_MODEL`).
    pub fn from_env(profile: &'static HttpProfile) -> Result<Option<Self>, XhlError> {
        let base = std::env::var("OPENAI_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let key = std::env::var("OPENAI_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty());

        let (Some(base), Some(key)) = (base, key) else {
            return Ok(None);
        };

        let model = crate::native::runtime()
            .and_then(|c| c.llm_model())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| Self::DEFAULT_MODEL.to_owned());

        let client = HttpClient::new(profile)?;
        Ok(Some(Self::new(client, base, key, model)))
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    async fn generate(&self, prompt: &str, opts: GenOpts) -> Result<String, XhlError> {
        let url = format!("{}/chat/completions", self.base_url);
        let headers = vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            (
                "authorization".to_owned(),
                format!("Bearer {}", self.api_key),
            ),
        ];
        let body = serde_json::json!({
            "model": self.model,
            "messages": [{ "role": "user", "content": prompt }],
            "max_tokens": opts.max_tokens,
            "temperature": opts.temperature,
        });

        let resp = self.client.post_json(&url, &headers, &body).await?;
        if !resp.is_success() {
            // Pesan provider bisa panjang; dibatasi agar tidak membanjiri output.
            let detail: String = resp.body.chars().take(300).collect();
            return Err(XhlError::Internal(format!(
                "provider LLM menolak permintaan (HTTP {}): {detail}",
                resp.status
            )));
        }

        let val: serde_json::Value = resp.json()?;
        val.get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_owned())
            .ok_or_else(|| {
                XhlError::Internal("respons LLM tidak memuat choices[0].message.content".into())
            })
    }

    fn name(&self) -> &'static str {
        "openai-compat"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::headers::FIREFOX_133;

    #[tokio::test]
    async fn null_provider_selalu_gagal_dengan_petunjuk_env() {
        let err = NullProvider
            .generate("apa saja", GenOpts::default())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("OPENAI_BASE_URL"), "{msg}");
        assert!(msg.contains("OPENAI_API_KEY"), "{msg}");
        assert_eq!(NullProvider.name(), "null");
    }

    #[test]
    fn gen_opts_default_wajar() {
        let o = GenOpts::default();
        assert_eq!(o.max_tokens, 400);
        assert!((o.temperature - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn debug_provider_tidak_membocorkan_api_key() {
        let client = HttpClient::new(&FIREFOX_133).unwrap();
        let p =
            OpenAiCompatProvider::new(client, "https://api.contoh/v1", "KUNCI_RAHASIA", "model-x");
        let dumped = format!("{p:?}");
        assert!(!dumped.contains("KUNCI_RAHASIA"), "api key bocor: {dumped}");
        assert!(dumped.contains("redacted"));
        assert_eq!(p.model(), "model-x");
    }

    #[test]
    fn base_url_tanpa_slash_di_akhir() {
        let client = HttpClient::new(&FIREFOX_133).unwrap();
        let p = OpenAiCompatProvider::new(client, "https://api.contoh/v1/", "k", "m");
        assert_eq!(p.base_url, "https://api.contoh/v1");
    }
}
