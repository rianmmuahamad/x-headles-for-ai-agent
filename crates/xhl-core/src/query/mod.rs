pub mod discovery;

use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::error::XhlError;
use crate::http::Client as HttpClient;
use crate::store::StoreHandle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryId(pub String);

impl QueryId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct QueryRegistry {
    /// Aset bawaan (`known_queries.json`) — dipakai sebagai *fallback*, bukan
    /// sumber kebenaran. Isinya bisa basi setelah X mengganti bundle.
    builtin: HashMap<String, String>,
    /// Hash yang sudah terverifikasi/discovery; menimpa `builtin`.
    ids: RwLock<HashMap<String, String>>,
    store: Option<StoreHandle>,
    client: HttpClient,
    /// Header untuk permintaan halaman/bundle saat discovery.
    ///
    /// Discovery **wajib** memakai cookie sesi: halaman X versi anonim tidak
    /// mengirim daftar script yang memuat operasi GraphQL.
    discover_headers: RwLock<crate::http::client::HeaderMap>,
}

impl QueryRegistry {
    pub fn new(client: HttpClient, store: Option<StoreHandle>) -> Self {
        // Aset disimpan terpisah: hasil discovery/cache harus menang atas aset,
        // karena aset bisa basi (X mengganti `queryId` tanpa mengubah namanya).
        const BUILTIN: &str = include_str!("../../assets/known_queries.json");
        let builtin = serde_json::from_str::<HashMap<String, String>>(BUILTIN).unwrap_or_default();

        Self {
            builtin,
            ids: RwLock::new(HashMap::new()),
            store,
            client,
            discover_headers: RwLock::new(Vec::new()),
        }
    }

    pub async fn resolve(&self, operation: &str) -> Result<QueryId, XhlError> {
        // 1. Hasil discovery pada proses ini (paling dipercaya).
        {
            let r = self.ids.read().await;
            if let Some(id) = r.get(operation) {
                return Ok(QueryId(id.clone()));
            }
        }

        // 2. Cache dari discovery sebelumnya. Ini **menimpa** aset bawaan:
        //    aset bisa basi, dan memakai hash basi menghasilkan
        //    "GraphQL error: Internal server error" dari X.
        if let Some(store) = &self.store {
            if let Ok(pairs) = store.run(|s| s.query_ids()).await {
                let mut w = self.ids.write().await;
                for (k, v) in pairs {
                    w.insert(k, v);
                }
                if let Some(id) = w.get(operation) {
                    return Ok(QueryId(id.clone()));
                }
            }
        }

        // 3. Aset bawaan, hanya sebagai titik awal sebelum discovery pertama.
        if let Some(id) = self.builtin.get(operation) {
            return Ok(QueryId(id.clone()));
        }

        // 4. Operasi ini belum dikenal: coba discovery. Kegagalan discovery
        //    dilaporkan apa adanya — menyamarkannya sebagai "queryId usang" akan
        //    menyesatkan (masalahnya jangkauan/koneksi, bukan hash yang berubah).
        match self.refresh().await {
            Ok(0) => {
                return Err(XhlError::AntiBotStateUnavailable(format!(
                    "discovery bundle X tidak menemukan operasi apa pun; \
                     '{}' tidak dapat di-resolve",
                    operation
                )))
            }
            Ok(_) => {}
            Err(e) => return Err(e),
        }

        let r = self.ids.read().await;
        if let Some(id) = r.get(operation) {
            return Ok(QueryId(id.clone()));
        }
        Err(XhlError::QueryIdStale {
            operation: operation.to_string(),
        })
    }

    /// Setel header discovery (cookie sesi). Tanpa ini discovery akan kosong.
    pub fn set_discover_headers(&self, headers: crate::http::client::HeaderMap) {
        if let Ok(mut w) = self.discover_headers.try_write() {
            *w = headers;
        }
    }

    pub async fn refresh(&self) -> Result<usize, XhlError> {
        let headers = self.discover_headers.read().await.clone();
        let discovered = discovery::discover(&self.client, &headers).await?;
        let count = discovered.len();
        if count == 0 {
            return Ok(0);
        }

        let mut w = self.ids.write().await;
        for (k, v) in &discovered {
            w.insert(k.clone(), v.clone());
            if let Some(store) = &self.store {
                let k_clone = k.clone();
                let v_clone = v.clone();
                let _ = store
                    .run(move |s| s.set_query_id(&k_clone, &v_clone, "discovery"))
                    .await;
            }
        }
        Ok(count)
    }

    /// Semua query ID yang diketahui, dengan presedensi yang **sama** seperti
    /// [`resolve`](Self::resolve): aset bawaan < cache store < hasil discovery
    /// proses ini.
    ///
    /// Sebelumnya fungsi ini hanya membaca state in-process, sehingga proses
    /// baru (setiap pemanggilan CLI) melaporkan "0 operasi" padahal store
    /// memuat ratusan hasil discovery — dan `xhl doctor` ikut melaporkan
    /// operasi inti sebagai "belum ter-resolve".
    pub async fn known(&self) -> Vec<(String, String)> {
        let mut merged: HashMap<String, String> = self.builtin.clone();
        if let Some(store) = &self.store {
            if let Ok(pairs) = store.run(|s| s.query_ids()).await {
                for (k, v) in pairs {
                    merged.insert(k, v);
                }
            }
        }
        {
            let r = self.ids.read().await;
            for (k, v) in r.iter() {
                merged.insert(k.clone(), v.clone());
            }
        }
        let mut list: Vec<_> = merged.into_iter().collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }

    pub fn set_override(&self, map: HashMap<String, String>) {
        if let Ok(mut w) = self.ids.try_write() {
            for (k, v) in map {
                w.insert(k, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::headers::FIREFOX_133;

    /// Registry yang tidak akan pernah melakukan discovery: klien menunjuk ke
    /// alamat yang tidak dapat dijangkau, sehingga setiap percobaan jaringan
    /// gagal cepat dan deterministik.
    fn offline_registry(store: Option<StoreHandle>) -> QueryRegistry {
        let client = HttpClient::new(&FIREFOX_133).expect("klien http");
        QueryRegistry::new(client, store)
    }

    #[tokio::test]
    async fn known_membaca_cache_store_bukan_hanya_state_proses() {
        // Regresi: proses baru (setiap pemanggilan CLI) harus melihat hasil
        // discovery sebelumnya. Sebelumnya `known()` hanya membaca state
        // in-process sehingga melaporkan 0 operasi walau store penuh.
        let store = StoreHandle::in_memory().expect("store");
        store
            .run(|s| s.set_query_id("SearchTimeline", "hash-dari-discovery", "discovery"))
            .await
            .expect("simpan query id");

        let reg = offline_registry(Some(store));
        let list = reg.known().await;
        let found = list
            .iter()
            .find(|(k, _)| k == "SearchTimeline")
            .map(|(_, v)| v.clone());
        assert_eq!(
            found.as_deref(),
            Some("hash-dari-discovery"),
            "cache store harus menang atas aset bawaan"
        );
        // Aset tetap ikut terbaca, jadi daftarnya lebih dari satu entri.
        assert!(list.len() > 1, "aset bawaan tetap disertakan");
    }

    #[tokio::test]
    async fn aset_dipakai_sebelum_discovery_pertama() {
        let reg = offline_registry(None);

        // Operasi di aset ter-resolve tanpa jaringan — aset adalah titik awal
        // sebelum discovery pertama, bukan sumber kebenaran.
        assert_eq!(
            reg.resolve("SearchTimeline").await.unwrap().as_str(),
            "hyPfJYJ_XAtDYoslQc-Rgg"
        );
        assert_eq!(
            reg.resolve("UserByScreenName").await.unwrap().as_str(),
            "Gb-d6r0vxPOADdG62OEBpQ"
        );
        assert_eq!(
            reg.resolve("TweetDetail").await.unwrap().as_str(),
            "XMOz5h24KAZ86qKffKTLdQ"
        );
    }

    /// Regresi: cache discovery **harus** menang atas aset bawaan.
    ///
    /// Aset `known_queries.json` bisa basi; memakai hash basi membuat X menjawab
    /// "GraphQL error: Internal server error". Bug ini pernah terjadi karena
    /// aset dimuat lebih dulu lalu dipakai lewat fast-path memori.
    #[tokio::test]
    async fn cache_discovery_menimpa_aset_bawaan() {
        let store = crate::store::StoreHandle::in_memory().unwrap();
        let fresh = "S_hzVUv1trgZ_5ruDe2IoA";
        {
            let s = store.clone();
            s.run(move |st| st.set_query_id("GenericTimelineById", fresh, "discovery"))
                .await
                .unwrap();
        }

        let reg = offline_registry(Some(store));

        // Aset bawaan memuat hash lama; hasil discovery harus dipakai.
        let resolved = reg.resolve("GenericTimelineById").await.unwrap();
        assert_eq!(
            resolved.as_str(),
            fresh,
            "hash dari discovery harus menang atas aset bawaan"
        );
        assert_ne!(resolved.as_str(), "ee4dBLWL8a8qg6n19m1htQ");
    }

    #[tokio::test]
    async fn set_override_menimpa_aset() {
        let reg = offline_registry(None);
        let mut map = HashMap::new();
        map.insert("CreateTweet".to_string(), "override123".to_string());
        reg.set_override(map);

        assert_eq!(
            reg.resolve("CreateTweet").await.unwrap().as_str(),
            "override123"
        );
    }

    #[tokio::test]
    async fn operasi_tak_dikenal_tanpa_jaringan_gagal_dengan_penyebab_sebenarnya() {
        let reg = offline_registry(None);

        // Discovery tidak dapat dijalankan (tanpa jaringan). Error harus
        // menjelaskan penyebab sebenarnya, BUKAN "queryId usang".
        let err = reg.resolve("OperasiYangTidakAdaDiAset").await.unwrap_err();
        assert!(
            !matches!(err, XhlError::QueryIdStale { .. }),
            "kegagalan discovery tidak boleh menyamar sebagai queryId usang: {err}"
        );
    }
}
