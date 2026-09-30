//! Operasi baca.

use crate::domain::{
    Handle, Page, SearchQuery, SearchRanking, TimelineKind, Trend, TrendCategory, Tweet, TweetId,
    UserProfile,
};
use crate::driver::XReader;
use crate::error::XhlError;

pub struct ResearchService<D> {
    driver: D,
}

impl<D: XReader> ResearchService<D> {
    pub fn new(driver: D) -> Self {
        Self { driver }
    }

    pub fn driver(&self) -> &D {
        &self.driver
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<Tweet>, XhlError> {
        if query.trim().is_empty() {
            return Err(XhlError::Invalid("query pencarian kosong".into()));
        }
        if limit == 0 {
            return Err(XhlError::Invalid("limit harus > 0".into()));
        }
        self.driver.search(&SearchQuery::new(query, limit)).await
    }

    pub async fn timeline(
        &self,
        kind: TimelineKind,
        handle: Option<&Handle>,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<Page<Tweet>, XhlError> {
        if kind == TimelineKind::User && handle.is_none() {
            return Err(XhlError::Invalid("timeline user memerlukan handle".into()));
        }
        if limit == 0 {
            return Err(XhlError::Invalid("limit harus > 0".into()));
        }
        self.driver.timeline(kind, handle, cursor, limit).await
    }

    pub async fn thread(&self, id: &TweetId) -> Result<Vec<Tweet>, XhlError> {
        let tweets = self.driver.thread(id).await?;
        if tweets.is_empty() {
            return Err(XhlError::Internal(format!("thread {} kosong", id.0)));
        }
        Ok(tweets)
    }

    pub async fn user(&self, handle: &Handle) -> Result<UserProfile, XhlError> {
        self.driver.user_by_handle(handle).await
    }

    /// Pencarian dengan ranking terpilih (`Top` = yang paling rame).
    pub async fn search_ranked(
        &self,
        query: &str,
        limit: usize,
        ranking: SearchRanking,
    ) -> Result<Vec<Tweet>, XhlError> {
        if query.trim().is_empty() {
            return Err(XhlError::Invalid("query pencarian kosong".into()));
        }
        if limit == 0 {
            return Err(XhlError::Invalid("limit harus > 0".into()));
        }
        self.driver
            .search(&SearchQuery::ranked(query, limit, ranking))
            .await
    }

    /// Topik yang sedang ramai di X.
    pub async fn trends(
        &self,
        category: TrendCategory,
        limit: usize,
    ) -> Result<Vec<Trend>, XhlError> {
        if limit == 0 {
            return Err(XhlError::Invalid("limit harus > 0".into()));
        }
        self.driver.trends(category, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::FakeDriver;

    #[tokio::test]
    async fn pencarian_kosong_atau_limit_nol_ditolak() {
        let svc = ResearchService::new(FakeDriver::with_session("contoh"));
        assert!(svc.search("  ", 5).await.is_err());
        assert!(svc.search("rust", 0).await.is_err());
        assert_eq!(svc.search("rust", 2).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn timeline_user_wajib_handle() {
        let svc = ResearchService::new(FakeDriver::with_session("contoh"));
        assert!(svc
            .timeline(TimelineKind::User, None, None, 5)
            .await
            .is_err());
        assert!(svc
            .timeline(TimelineKind::User, Some(&Handle::parse("jack")), None, 5)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn timeline_menghormati_limit() {
        let svc = ResearchService::new(FakeDriver::with_session("contoh"));
        let page = svc
            .timeline(TimelineKind::Home, None, None, 3)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 3);
        assert!(svc
            .timeline(TimelineKind::Home, None, None, 0)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn search_ranked_dan_trends_menegakkan_validasi() {
        let svc = ResearchService::new(FakeDriver::with_session("contoh"));

        assert!(svc
            .search_ranked("  ", 5, SearchRanking::Top)
            .await
            .is_err());
        assert!(svc
            .search_ranked("rust", 0, SearchRanking::Top)
            .await
            .is_err());
        assert_eq!(
            svc.search_ranked("rust", 2, SearchRanking::Top)
                .await
                .unwrap()
                .len(),
            2
        );

        assert!(svc.trends(TrendCategory::Trending, 0).await.is_err());
        assert_eq!(svc.trends(TrendCategory::News, 4).await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn user_mengembalikan_profil() {
        let svc = ResearchService::new(FakeDriver::with_session("contoh"));
        let u = svc.user(&Handle::parse("jack")).await.unwrap();
        assert_eq!(u.handle.0, "jack");
    }
}
