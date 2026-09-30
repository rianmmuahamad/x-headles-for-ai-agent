use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use time::OffsetDateTime;

use crate::error::XhlError;

#[derive(Debug, Clone)]
struct Bucket {
    remaining: u32,
    reset_at: Option<OffsetDateTime>,
    penalty_until: Option<OffsetDateTime>,
    consecutive_429: u32,
}

impl Default for Bucket {
    fn default() -> Self {
        Self {
            remaining: 50, // default buffer awal
            reset_at: None,
            penalty_until: None,
            consecutive_429: 0,
        }
    }
}

pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Tunggu atau dapatkan izin untuk melakukan request pada bucket tertentu.
    pub async fn acquire(&self, bucket: &str, no_wait: bool) -> Result<(), XhlError> {
        let now = OffsetDateTime::now_utc();
        let wait_dur = {
            let mut guard = self
                .buckets
                .lock()
                .map_err(|_| XhlError::Internal("limiter lock poisoned".into()))?;
            let b = guard.entry(bucket.to_string()).or_default();

            // Cek penalti aktif lebih dulu
            if let Some(penalty) = b.penalty_until {
                if penalty > now {
                    let diff = (penalty - now).unsigned_abs();
                    Some(diff)
                } else {
                    b.penalty_until = None;
                    b.consecutive_429 = 0;
                    None
                }
            } else if b.remaining == 0 {
                if let Some(reset) = b.reset_at {
                    if reset > now {
                        let diff = (reset - now).unsigned_abs();
                        Some(diff)
                    } else {
                        b.remaining = 1;
                        b.reset_at = None;
                        None
                    }
                } else {
                    None
                }
            } else {
                b.remaining = b.remaining.saturating_sub(1);
                None
            }
        };

        if let Some(dur) = wait_dur {
            if no_wait {
                return Err(XhlError::RateLimited { retry_after: dur });
            }
            tracing::info!(bucket = %bucket, wait_secs = dur.as_secs(), "rate limit tercapai, tidur...");
            tokio::time::sleep(dur).await;
            // Pulihkan satu token setelah menunggu
            if let Ok(mut guard) = self.buckets.lock() {
                if let Some(b) = guard.get_mut(bucket) {
                    b.remaining = 1;
                }
            }
        }

        Ok(())
    }

    /// Perbarui kuota dan waktu reset dari header HTTP respons X.
    pub fn observe(&self, bucket: &str, headers: &[(String, String)]) {
        let mut remaining_opt = None;
        let mut reset_opt = None;

        for (k, v) in headers {
            if k.eq_ignore_ascii_case("x-rate-limit-remaining") {
                if let Ok(val) = v.parse::<u32>() {
                    remaining_opt = Some(val);
                }
            } else if k.eq_ignore_ascii_case("x-rate-limit-reset") {
                if let Ok(epoch_secs) = v.parse::<i64>() {
                    if let Ok(dt) = OffsetDateTime::from_unix_timestamp(epoch_secs) {
                        reset_opt = Some(dt);
                    }
                }
            }
        }

        if let Ok(mut guard) = self.buckets.lock() {
            let b = guard.entry(bucket.to_string()).or_default();
            if let Some(r) = remaining_opt {
                b.remaining = r;
            }
            if let Some(res) = reset_opt {
                b.reset_at = Some(res);
            }
        }
    }

    /// Terapkan penalti eksponensial saat menerima status 429
    pub fn penalize(&self, bucket: &str) -> Duration {
        let now = OffsetDateTime::now_utc();
        let mut guard = match self.buckets.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };

        let b = guard.entry(bucket.to_string()).or_default();
        b.consecutive_429 += 1;
        // 30s * 2^(consecutive - 1), capped 900s (15 menit)
        let multiplier = 1u64
            .checked_shl(b.consecutive_429.saturating_sub(1).min(5))
            .unwrap_or(32);
        let backoff_secs = (30 * multiplier).min(900);
        let dur = Duration::from_secs(backoff_secs);
        b.penalty_until = Some(now + dur);
        dur
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn observe_dan_acquire_menghormati_remaining_nol() {
        let limiter = RateLimiter::new();
        let future_reset = (OffsetDateTime::now_utc() + Duration::from_secs(60)).unix_timestamp();
        let headers = vec![
            ("x-rate-limit-remaining".to_string(), "0".to_string()),
            ("x-rate-limit-reset".to_string(), future_reset.to_string()),
        ];

        limiter.observe("SearchTimeline", &headers);

        // Dengan no_wait = true, harus langsung Err(RateLimited)
        let err = limiter.acquire("SearchTimeline", true).await.unwrap_err();
        match err {
            XhlError::RateLimited { retry_after } => {
                assert!(retry_after.as_secs() <= 60);
                assert!(retry_after.as_secs() >= 55);
            }
            _ => panic!("harus RateLimited"),
        }
    }

    #[test]
    fn penalize_eksponensial() {
        let limiter = RateLimiter::new();
        let d1 = limiter.penalize("op1");
        assert_eq!(d1, Duration::from_secs(30));

        let d2 = limiter.penalize("op1");
        assert_eq!(d2, Duration::from_secs(60));

        let d3 = limiter.penalize("op1");
        assert_eq!(d3, Duration::from_secs(120));
    }
}
