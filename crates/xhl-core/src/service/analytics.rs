//! Analytics: ambil metrics, simpan snapshot, hitung delta antar snapshot.

use std::time::Duration;
use time::OffsetDateTime;

use crate::domain::{Metrics, TweetId};
use crate::driver::XReader;
use crate::error::XhlError;
use crate::store::{format_rfc3339, StoreHandle};

/// Satu titik waktu pengambilan metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSnapshot {
    pub captured_at: OffsetDateTime,
    pub metrics: Metrics,
}

/// Selisih metrics antara snapshot terlama dan terbaru.
///
/// Field `i64` karena metrics bisa turun (unlike, view stabil).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsDelta {
    pub likes: i64,
    pub reposts: i64,
    pub replies: i64,
    pub views: i64,
    pub bookmarks: i64,
    pub span: Duration,
}

impl MetricsDelta {
    /// Bangun delta dari dua snapshot. `None` bila rentang waktunya negatif.
    pub fn between(from: &MetricsSnapshot, to: &MetricsSnapshot) -> Option<Self> {
        let span = (to.captured_at - from.captured_at).try_into().ok()?;
        Some(Self {
            likes: diff(from.metrics.likes, to.metrics.likes),
            reposts: diff(from.metrics.reposts, to.metrics.reposts),
            replies: diff(from.metrics.replies, to.metrics.replies),
            views: diff(from.metrics.views, to.metrics.views),
            bookmarks: diff(from.metrics.bookmarks, to.metrics.bookmarks),
            span,
        })
    }
}

/// Selisih dua nilai opsional. Field yang absen di salah satu sisi dianggap 0.
fn diff(from: Option<u64>, to: Option<u64>) -> i64 {
    to.unwrap_or(0) as i64 - from.unwrap_or(0) as i64
}

pub struct AnalyticsService<D> {
    driver: D,
    store: StoreHandle,
}

impl<D: XReader> AnalyticsService<D> {
    pub fn new(driver: D, store: StoreHandle) -> Self {
        Self { driver, store }
    }

    /// Ambil metrics dari X dan simpan sebagai snapshot.
    pub async fn capture(&self, id: &TweetId) -> Result<Metrics, XhlError> {
        let metrics = self.driver.tweet_metrics(id).await?;
        let store = self.store.clone();
        let tweet_id = id.0.clone();
        let to_save = metrics.clone();
        store
            .run(move |s| {
                s.insert_metrics(&tweet_id, &to_save, OffsetDateTime::now_utc())?;
                Ok(())
            })
            .await?;
        Ok(metrics)
    }

    /// Snapshot lokal, terbaru lebih dulu.
    pub async fn history(
        &self,
        id: &TweetId,
        limit: usize,
    ) -> Result<Vec<MetricsSnapshot>, XhlError> {
        if limit == 0 {
            return Err(XhlError::Invalid("limit harus > 0".into()));
        }
        let store = self.store.clone();
        let tweet_id = id.0.clone();
        let rows = store
            .run(move |s| s.metrics_history(&tweet_id, limit))
            .await?;

        Ok(rows
            .into_iter()
            .map(|(captured_at, metrics)| MetricsSnapshot {
                captured_at,
                metrics,
            })
            .collect())
    }

    /// Delta antara snapshot terbaru dan terlama yang tersimpan.
    ///
    /// `None` bila belum ada minimal dua snapshot — pemanggil harus memberi tahu
    /// pengguna untuk mengambil snapshot lagi, bukan menampilkan angka 0 menyesatkan.
    pub async fn delta(
        &self,
        id: &TweetId,
        limit: usize,
    ) -> Result<Option<MetricsDelta>, XhlError> {
        let history = self.history(id, limit).await?;
        if history.len() < 2 {
            return Ok(None);
        }
        let newest = &history[0];
        let oldest = &history[history.len() - 1];
        Ok(MetricsDelta::between(oldest, newest))
    }

    /// Snapshot terakhir, bila ada.
    pub async fn latest(&self, id: &TweetId) -> Result<Option<MetricsSnapshot>, XhlError> {
        Ok(self.history(id, 1).await?.into_iter().next())
    }

    /// Rentang waktu antar snapshot terakhir dengan yang sebelumnya.
    pub async fn snapshot_count(&self, id: &TweetId) -> Result<usize, XhlError> {
        let history = self.history(id, 10_000).await?;
        Ok(history.len())
    }
}

/// Format snapshot untuk ditampilkan adapter.
pub fn snapshot_line(s: &MetricsSnapshot) -> String {
    let f = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_else(|| "-".to_owned());
    format!(
        "{}  likes={} reposts={} replies={} views={} bookmarks={}",
        format_rfc3339(s.captured_at),
        f(s.metrics.likes),
        f(s.metrics.reposts),
        f(s.metrics.replies),
        f(s.metrics.views),
        f(s.metrics.bookmarks),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::FakeDriver;
    use crate::store::StoreHandle;

    fn svc(driver: FakeDriver, store: StoreHandle) -> AnalyticsService<FakeDriver> {
        AnalyticsService::new(driver, store)
    }

    #[tokio::test]
    async fn capture_menyimpan_snapshot() {
        let store = StoreHandle::in_memory().unwrap();
        let svc = svc(FakeDriver::with_session("contoh"), store);

        svc.capture(&TweetId("111".into())).await.unwrap();
        assert_eq!(svc.snapshot_count(&TweetId("111".into())).await.unwrap(), 1);

        svc.capture(&TweetId("111".into())).await.unwrap();
        let history = svc.history(&TweetId("111".into()), 10).await.unwrap();
        assert_eq!(
            history.len(),
            2,
            "snapshot dengan timestamp berbeda harus tersimpan"
        );
    }

    #[tokio::test]
    async fn delta_kosong_bila_kurang_dua_snapshot() {
        let store = StoreHandle::in_memory().unwrap();
        let svc = svc(FakeDriver::with_session("contoh"), store);
        let id = TweetId("222".into());

        assert_eq!(svc.delta(&id, 10).await.unwrap(), None);
        svc.capture(&id).await.unwrap();
        assert_eq!(
            svc.delta(&id, 10).await.unwrap(),
            None,
            "satu snapshot belum cukup untuk delta"
        );
    }

    #[test]
    fn delta_menghitung_selisih_termasuk_negatif() {
        let from = MetricsSnapshot {
            captured_at: OffsetDateTime::UNIX_EPOCH,
            metrics: Metrics {
                likes: Some(10),
                reposts: Some(5),
                replies: None,
                views: Some(1000),
                bookmarks: Some(1),
            },
        };
        let to = MetricsSnapshot {
            captured_at: OffsetDateTime::UNIX_EPOCH + Duration::from_secs(3600),
            metrics: Metrics {
                likes: Some(7),
                reposts: Some(9),
                replies: Some(2),
                views: Some(1500),
                bookmarks: None,
            },
        };

        let d = MetricsDelta::between(&from, &to).expect("delta valid");
        assert_eq!(d.likes, -3, "unlike harus terlihat sebagai delta negatif");
        assert_eq!(d.reposts, 4);
        assert_eq!(d.replies, 2, "field yang absen dihitung 0");
        assert_eq!(d.views, 500);
        assert_eq!(d.bookmarks, -1);
        assert_eq!(d.span, Duration::from_secs(3600));
    }

    #[test]
    fn delta_menolak_urutan_waktu_terbalik() {
        let a = MetricsSnapshot {
            captured_at: OffsetDateTime::UNIX_EPOCH,
            metrics: Metrics::default(),
        };
        let b = MetricsSnapshot {
            captured_at: OffsetDateTime::UNIX_EPOCH - Duration::from_secs(60),
            metrics: Metrics::default(),
        };
        assert!(MetricsDelta::between(&a, &b).is_none());
    }

    #[tokio::test]
    async fn history_nol_ditolak() {
        let store = StoreHandle::in_memory().unwrap();
        let svc = svc(FakeDriver::with_session("contoh"), store);
        assert!(svc.history(&TweetId("1".into()), 0).await.is_err());
    }
}
