//! Error domain. Setiap varian punya pemetaan eksplisit: retryable / butuh manusia /
//! aman untuk agent. Adapter hanya memetakan ke protokolnya, tanpa mengubah semantik.

use std::time::Duration;

/// Alasan di balik 403. Memisahkan sebab agar aksi pemulihannya tepat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForbiddenReason {
    /// `ct0` cookie dan header `x-csrf-token` tidak sinkron.
    Csrf,
    /// `x-client-transaction-id` salah atau state-nya basi.
    TransactionId,
    /// Fingerprint TLS / `user-agent` tidak konsisten.
    Tls,
    /// Sesi ditandai berisiko oleh X.
    Suspicious,
    Unknown,
}

impl ForbiddenReason {
    /// Klasifikasi dari sandi status + body respons X.
    pub fn classify(status: u16, body: &str) -> Self {
        let lower = body.to_ascii_lowercase();
        if status == 401 {
            return Self::Csrf;
        }
        if lower.contains("bad csrf") || lower.contains("csrf") {
            return Self::Csrf;
        }
        if lower.contains("transaction") || lower.contains("client-transaction") {
            return Self::TransactionId;
        }
        if lower.contains("suspicious") || lower.contains("automated") || lower.contains("bot") {
            return Self::Suspicious;
        }
        Self::Unknown
    }
}

#[derive(Debug, thiserror::Error)]
pub enum XhlError {
    #[error("session kedaluwarsa: {0}")]
    AuthExpired(String),

    #[error("dilarang ({reason:?}): {detail}")]
    Forbidden {
        reason: ForbiddenReason,
        detail: String,
    },

    #[error("queryId '{operation}' usang")]
    QueryIdStale { operation: String },
    #[error("state anti-bot tidak tersedia: {0}")]
    AntiBotStateUnavailable(String),

    #[error("rate limited, reset dalam {retry_after:?}")]
    RateLimited { retry_after: Duration },

    #[error("network: {0}")]
    Network(String),

    #[error("timeout pada {stage}")]
    Timeout { stage: &'static str },

    #[error("hasil tidak diketahui untuk aksi write: {hint}")]
    UnknownOutcome { hint: String },

    #[error("validasi: {0}")]
    Invalid(String),

    #[error("konfigurasi: {0}")]
    Config(String),

    #[error("operasi belum diimplementasikan: {0}")]
    NotImplemented(&'static str),

    #[error("internal: {0}")]
    Internal(String),
}

impl XhlError {
    /// Boleh dicoba ulang otomatis tanpa mengubah kondisi eksternal?
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::Timeout { .. } | Self::QueryIdStale { .. }
        )
    }

    /// Perlu tindakan manusia (tidak akan sembuh dengan retry)?
    pub fn needs_human(&self) -> bool {
        matches!(
            self,
            Self::AuthExpired(_)
                | Self::AntiBotStateUnavailable(_)
                | Self::Forbidden {
                    reason: ForbiddenReason::Suspicious | ForbiddenReason::Tls,
                    ..
                }
                | Self::Config(_)
        )
    }

    /// Aman diberitahukan ke agent (tidak membocorkan kredensial/detail internal)?
    pub fn agent_safe_message(&self) -> String {
        match self {
            Self::AuthExpired(_) => {
                "Session X tidak valid. Minta pengguna menjalankan: xhl auth import".to_owned()
            }
            Self::Forbidden { reason, .. } => format!(
                "Permintaan ditolak oleh X ({reason:?}). Lihat 'xhl doctor' untuk diagnosis."
            ),
            Self::RateLimited { retry_after } => {
                format!(
                    "Rate limit X tercapai; coba lagi dalam {}s.",
                    retry_after.as_secs()
                )
            }
            Self::UnknownOutcome { hint } => format!(
                "Status aksi write tidak diketahui — verifikasi manual sebelum mengulang. {hint}"
            ),
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn klasifikasi_403_csrf() {
        assert_eq!(
            ForbiddenReason::classify(403, "Bad CSRF Token"),
            ForbiddenReason::Csrf
        );
    }

    #[test]
    fn klasifikasi_403_transaction() {
        assert_eq!(
            ForbiddenReason::classify(403, "invalid client-transaction-id"),
            ForbiddenReason::TransactionId
        );
    }

    #[test]
    fn klasifikasi_401_selalu_csrf() {
        assert_eq!(ForbiddenReason::classify(401, ""), ForbiddenReason::Csrf);
    }

    #[test]
    fn unknown_outcome_tidak_retryable() {
        let e = XhlError::UnknownOutcome {
            hint: "post".into(),
        };
        assert!(!e.is_retryable());
        assert!(!e.needs_human());
    }

    #[test]
    fn auth_expired_butuh_manusia() {
        assert!(XhlError::AuthExpired("no cookie".into()).needs_human());
    }
}
