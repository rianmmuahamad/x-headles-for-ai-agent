use super::client::HeaderMap;
use crate::session::{Cookies, WEB_BEARER};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpProfile {
    pub name: &'static str,
    pub user_agent: &'static str,
    pub omit_user_agent: bool,
}

/// User-agent Firefox 133. Dipakai saat impersonasi TLS **tidak** aktif; dengan
/// impersonasi, `rquest` menyetel UA sendiri sesuai profil TLS dan kita tidak
/// boleh mengirim UA terpisah (inkonsistensi justru menandai bot).
pub const UA_FIREFOX_133: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0";
pub const UA_CHROME_131: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

#[cfg(not(feature = "impersonate"))]
pub const FIREFOX_133: HttpProfile = HttpProfile {
    name: "firefox_133",
    user_agent: UA_FIREFOX_133,
    omit_user_agent: false,
};

#[cfg(feature = "impersonate")]
pub const FIREFOX_133: HttpProfile = HttpProfile {
    name: "firefox_133",
    user_agent: UA_FIREFOX_133,
    omit_user_agent: true,
};

#[cfg(not(feature = "impersonate"))]
pub const CHROME_131: HttpProfile = HttpProfile {
    name: "chrome_131",
    user_agent: UA_CHROME_131,
    omit_user_agent: false,
};

#[cfg(feature = "impersonate")]
pub const CHROME_131: HttpProfile = HttpProfile {
    name: "chrome_131",
    user_agent: UA_CHROME_131,
    omit_user_agent: true,
};

pub const PROFILES: &[HttpProfile] = &[FIREFOX_133, CHROME_131];

pub fn profile_by_name(name: &str) -> Option<&'static HttpProfile> {
    PROFILES.iter().find(|p| p.name == name)
}

pub struct RequestHeaders<'a> {
    pub cookies: &'a Cookies,
    pub profile: &'static HttpProfile,
    pub transaction_id: Option<&'a str>,
    pub json_body: bool,
}

pub fn build(h: RequestHeaders<'_>) -> HeaderMap {
    build_with_overrides(h, crate::native::header_overrides())
}

/// Versi murni dari [`build`]: override datang sebagai argumen, bukan dari
/// state proses. Semua perilaku override karena itu dapat diuji tanpa
/// menyentuh environment.
pub fn build_with_overrides(
    h: RequestHeaders<'_>,
    overrides: Option<&crate::native::HeaderOverrides>,
) -> HeaderMap {
    let mut map = Vec::new();
    map.push(("authorization".to_string(), format!("Bearer {WEB_BEARER}")));
    map.push((
        "x-csrf-token".to_string(),
        h.cookies.csrf_token().to_string(),
    ));
    map.push((
        "x-twitter-auth-type".to_string(),
        "OAuth2Session".to_string(),
    ));
    map.push(("x-twitter-active-user".to_string(), "yes".to_string()));
    map.push(("x-twitter-client-language".to_string(), "en".to_string()));

    // `omit_user_agent` ada karena saat impersonasi TLS aktif, `wreq` menyetel
    // UA sendiri sesuai profil TLS dan UA terpisah justru inkonsisten. Tetapi
    // bila profil anti-detect menyetel `headers.User-Agent`, nilai itu memang
    // dipilih agar konsisten dengan identitas yang diklaim — jadi profil yang
    // menang, bukan `omit_user_agent`.
    let profile_ua = overrides.and_then(|o| o.user_agent.as_deref());
    if profile_ua.is_none() && !h.profile.omit_user_agent {
        map.push(("user-agent".to_string(), h.profile.user_agent.to_string()));
    }

    if h.json_body {
        map.push(("content-type".to_string(), "application/json".to_string()));
    }

    if let Some(tid) = h.transaction_id {
        map.push(("x-client-transaction-id".to_string(), tid.to_string()));
    }

    map.push(("cookie".to_string(), h.cookies.as_cookie_header()));
    apply_overrides_to(map, overrides)
}

/// Terapkan header turunan profil anti-detect (`XHL_CONFIG.profile`) bila ada.
///
/// Perilaku yang ditiru dari camoufox (`network-patches.patch` menambal
/// nsHttpHandler dan membaca `headers.*` dari config): nilai profil menang atas
/// nilai bawaan, dan urutan header akhir mengikuti `headers.order`.
///
/// Header yang tidak disebut `order` **tidak dihapus** — hanya ditempatkan
/// setelah yang disebut, dalam urutan aslinya. Header wajib (`cookie`,
/// `x-csrf-token`, `authorization`) karenanya tidak mungkin hilang hanya karena
/// profil tidak menyebutkannya.
pub fn apply_profile_overrides(map: HeaderMap) -> HeaderMap {
    apply_overrides_to(map, crate::native::header_overrides())
}

/// Versi murni dari [`apply_profile_overrides`]: override sebagai argumen.
pub fn apply_overrides_to(
    mut map: HeaderMap,
    overrides: Option<&crate::native::HeaderOverrides>,
) -> HeaderMap {
    let Some(ov) = overrides else {
        return map;
    };

    for (key, value) in [
        ("user-agent", ov.user_agent.as_deref()),
        ("accept-language", ov.accept_language.as_deref()),
        ("accept-encoding", ov.accept_encoding.as_deref()),
        ("accept", ov.accept.as_deref()),
        ("dnt", ov.dnt.as_deref()),
        ("viewport-width", ov.viewport_width.as_deref()),
        ("sec-ch-ua", ov.sec_ch_ua.as_deref()),
        ("sec-ch-ua-mobile", ov.sec_ch_ua_mobile.as_deref()),
        ("sec-ch-ua-platform", ov.sec_ch_ua_platform.as_deref()),
    ] {
        let Some(value) = value else { continue };
        match map.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value.to_owned(),
            None => map.push((key.to_owned(), value.to_owned())),
        }
    }

    if ov.order.is_empty() {
        return map;
    }
    // Urutan akhir: kunci yang disebut `order` lebih dulu (menurut `order`),
    // sisanya menyusul dalam urutan semula.
    let mut ordered: HeaderMap = Vec::with_capacity(map.len());
    for name in &ov.order {
        let name = name.to_ascii_lowercase();
        for (k, v) in &map {
            if *k == name && !ordered.iter().any(|(ek, _)| ek == k) {
                ordered.push((k.clone(), v.clone()));
            }
        }
    }
    for (k, v) in &map {
        if !ordered.iter().any(|(ek, _)| ek == k) {
            ordered.push((k.clone(), v.clone()));
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uji_wajib_b3_header_builder() {
        let cookies = Cookies {
            auth_token: "AT".to_string(),
            ct0: "CT".to_string(),
            twid: None,
            extras: vec![],
        };

        // 1 & 2 & 3: Entri cookie dan csrf
        let h1 = build(RequestHeaders {
            cookies: &cookies,
            profile: &FIREFOX_133,
            transaction_id: None,
            json_body: false,
        });

        let cookie_entries: Vec<_> = h1.iter().filter(|(k, _)| k == "cookie").collect();
        assert_eq!(cookie_entries.len(), 1, "harus ada tepat satu entri cookie");
        assert_eq!(cookie_entries[0].1, "auth_token=AT; ct0=CT");

        let csrf_entries: Vec<_> = h1.iter().filter(|(k, _)| k == "x-csrf-token").collect();
        assert_eq!(
            csrf_entries.len(),
            1,
            "harus ada tepat satu entri x-csrf-token"
        );
        assert_eq!(csrf_entries[0].1, "CT");

        let auth_entries: Vec<_> = h1.iter().filter(|(k, _)| k == "authorization").collect();
        assert_eq!(auth_entries.len(), 1);
        assert_eq!(auth_entries[0].1, format!("Bearer {WEB_BEARER}"));
        assert!(!auth_entries[0].1.contains("CT"));

        // 4: json_body
        let h_no_json = build(RequestHeaders {
            cookies: &cookies,
            profile: &FIREFOX_133,
            transaction_id: None,
            json_body: false,
        });
        assert!(!h_no_json.iter().any(|(k, _)| k == "content-type"));

        let h_json = build(RequestHeaders {
            cookies: &cookies,
            profile: &FIREFOX_133,
            transaction_id: None,
            json_body: true,
        });
        assert!(h_json
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/json"));

        // 5: transaction_id None vs Some
        assert!(!h1.iter().any(|(k, _)| k == "x-client-transaction-id"));
        let h_tid = build(RequestHeaders {
            cookies: &cookies,
            profile: &FIREFOX_133,
            transaction_id: Some("tx_123"),
            json_body: false,
        });
        assert_eq!(
            h_tid
                .iter()
                .find(|(k, _)| k == "x-client-transaction-id")
                .map(|(_, v)| v.as_str()),
            Some("tx_123")
        );

        // 6: profile.omit_user_agent
        static P_OMIT: HttpProfile = HttpProfile {
            name: "test_omit",
            user_agent: "test_ua",
            omit_user_agent: true,
        };
        let h_omit = build(RequestHeaders {
            cookies: &cookies,
            profile: &P_OMIT,
            transaction_id: None,
            json_body: false,
        });
        assert!(!h_omit.iter().any(|(k, _)| k == "user-agent"));

        // 7: format!("{headers:?}") tidak membocorkan AT di non-cookie entries
        for (k, v) in &h1 {
            if k != "cookie" {
                assert!(!v.contains("AT"), "kredensial bocor pada header {k}: {v}");
            }
        }
    }

    fn cookies() -> Cookies {
        Cookies {
            auth_token: "AT".to_string(),
            ct0: "CT".to_string(),
            twid: None,
            extras: vec![],
        }
    }

    /// `build` versi uji: tanpa override dari environment.
    ///
    /// Uji tidak boleh bergantung pada `XHL_CONFIG` yang kebetulan diset di
    /// mesin yang menjalankannya. Perilaku override diuji terpisah lewat
    /// `build_with_overrides`, sehingga keduanya hermetik.
    fn build(h: RequestHeaders<'_>) -> HeaderMap {
        super::build_with_overrides(h, None)
    }

    #[test]
    fn override_profil_menang_atas_omit_user_agent() {
        // Saat impersonasi aktif `omit_user_agent` = true; profil anti-detect
        // yang menyetel UA harus tetap menang, karena UA itulah yang konsisten
        // dengan identitas yang diklaim.
        static P_OMIT: HttpProfile = HttpProfile {
            name: "test_omit",
            user_agent: "bawaan",
            omit_user_agent: true,
        };
        let ov = crate::native::HeaderOverrides {
            user_agent: Some("UA-Profil".into()),
            accept_language: Some("id-ID,id;q=0.9".into()),
            order: vec!["cookie".into(), "user-agent".into()],
            from_profile: true,
            ..Default::default()
        };
        let h = build_with_overrides(
            RequestHeaders {
                cookies: &cookies(),
                profile: &P_OMIT,
                transaction_id: None,
                json_body: false,
            },
            Some(&ov),
        );

        let ua: Vec<_> = h.iter().filter(|(k, _)| k == "user-agent").collect();
        assert_eq!(ua.len(), 1, "tepat satu user-agent");
        assert_eq!(ua[0].1, "UA-Profil", "nilai profil menang");

        let al = h
            .iter()
            .find(|(k, _)| k == "accept-language")
            .expect("accept-language dari profil");
        assert_eq!(al.1, "id-ID,id;q=0.9");

        // Urutan: yang disebut `headers.order` lebih dulu.
        assert_eq!(h[0].0, "cookie");
        assert_eq!(h[1].0, "user-agent");
    }

    #[test]
    fn override_tidak_pernah_menghapus_header_wajib() {
        // Profil hanya menyebut satu header; cookie, csrf, dan authorization
        // harus tetap ada — header wajib tidak boleh hilang karena profil lupa
        // menyebutkannya.
        let ov = crate::native::HeaderOverrides {
            user_agent: Some("UA-Profil".into()),
            order: vec!["user-agent".into()],
            from_profile: true,
            ..Default::default()
        };
        let h = build_with_overrides(
            RequestHeaders {
                cookies: &cookies(),
                profile: &FIREFOX_133,
                transaction_id: Some("tx"),
                json_body: false,
            },
            Some(&ov),
        );
        for required in [
            "cookie",
            "x-csrf-token",
            "authorization",
            "x-client-transaction-id",
        ] {
            assert_eq!(
                h.iter().filter(|(k, _)| k == required).count(),
                1,
                "header wajib '{required}' harus tetap ada"
            );
        }
        // Nama yang tidak ada di `order` tetap hadir, setelah yang disebut.
        assert_eq!(h[0].0, "user-agent");
        assert!(h.iter().any(|(k, _)| k == "cookie"));
    }

    #[test]
    fn tanpa_override_perilaku_tidak_berubah() {
        let h = build_with_overrides(
            RequestHeaders {
                cookies: &cookies(),
                profile: &FIREFOX_133,
                transaction_id: None,
                json_body: false,
            },
            None,
        );
        let ua = h
            .iter()
            .find(|(k, _)| k == "user-agent")
            .expect("user-agent bawaan profil HTTP");
        assert_eq!(ua.1, FIREFOX_133.user_agent);
    }
}
