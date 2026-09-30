//! Session: parsing cookie, penyimpanan aman, dan health-check.
//!
//! Aturan keras: kredensial **tidak pernah** keluar lewat `Debug`, log, pesan error,
//! atau output tool. `Cookies` punya `Debug` manual yang meredaksi nilainya.

use serde::{Deserialize, Serialize};

use crate::error::XhlError;

/// Public bearer dari bundle web X. Bukan rahasia akun, tapi **versi-terikat**:
/// bila X menggantinya, semua request akan 401/403 dan konstanta ini harus diperbarui.
pub const WEB_BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";

/// Cookie sesi. Field kritis: `auth_token` (login) dan `ct0` (CSRF).
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cookies {
    pub auth_token: String,
    pub ct0: String,
    pub twid: Option<String>,
    /// Cookie lain yang ikut diimpor (guest_id, dll.) — dipertahankan apa adanya.
    #[serde(default)]
    pub extras: Vec<(String, String)>,
}

/// Redaksi total: tidak ada nilai cookie yang boleh tercetak.
impl std::fmt::Debug for Cookies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cookies")
            .field("auth_token", &"<redacted>")
            .field("ct0", &"<redacted>")
            .field("twid", &self.twid.as_ref().map(|_| "<redacted>"))
            .field("extras", &format!("<{} cookie>", self.extras.len()))
            .finish()
    }
}

impl Cookies {
    /// Nilai yang harus sama dengan header `x-csrf-token`.
    pub fn csrf_token(&self) -> &str {
        &self.ct0
    }

    /// Serialisasi header `cookie:` untuk request.
    pub fn as_cookie_header(&self) -> String {
        let mut out = format!("auth_token={}; ct0={}", self.auth_token, self.ct0);
        if let Some(twid) = &self.twid {
            out.push_str(&format!("; twid={twid}"));
        }
        for (k, v) in &self.extras {
            out.push_str(&format!("; {k}={v}"));
        }
        out
    }

    /// Parse string mentah `name=value; name2=value2` (format `twscrape add_cookie`).
    pub fn parse_header(raw: &str) -> Result<Self, XhlError> {
        let pairs = parse_pairs(raw);
        Self::from_pairs(pairs, "string cookie")
    }

    /// Parse ekspor JSON extension browser: array `[{name,value,…}]` atau objek `{name: value}`.
    pub fn parse_json(raw: &str) -> Result<Self, XhlError> {
        let value: serde_json::Value = serde_json::from_str(raw)
            .map_err(|e| XhlError::Invalid(format!("JSON cookie tidak valid: {e}")))?;

        let pairs: Vec<(String, String)> = match value {
            serde_json::Value::Array(items) => items
                .into_iter()
                .filter_map(|item| {
                    let name = item.get("name")?.as_str()?.to_owned();
                    let val = item.get("value")?.as_str()?.to_owned();
                    Some((name, val))
                })
                .collect(),
            serde_json::Value::Object(map) => map
                .into_iter()
                .filter_map(|(k, v)| Some((k, v.as_str()?.to_owned())))
                .collect(),
            _ => {
                return Err(XhlError::Invalid(
                    "JSON cookie harus array atau objek".into(),
                ))
            }
        };

        Self::from_pairs(pairs, "JSON cookie")
    }

    /// Parse file cookie Netscape (`cookies.txt`) hasil ekspor extension browser.
    pub fn parse_netscape(raw: &str) -> Result<Self, XhlError> {
        let mut pairs = Vec::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // domain \t flag \t path \t secure \t expiration \t name \t value
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() >= 7 {
                let name = fields[fields.len() - 2].trim();
                let val = fields[fields.len() - 1].trim();
                if !name.is_empty() {
                    pairs.push((name.to_owned(), val.to_owned()));
                }
            }
        }
        Self::from_pairs(pairs, "file Netscape")
    }

    /// Deteksi format secara otomatis dari isi.
    pub fn parse_auto(raw: &str) -> Result<Self, XhlError> {
        let trimmed = raw.trim_start();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            return Self::parse_json(raw);
        }
        if raw.lines().any(|l| l.split('\t').count() >= 7) {
            return Self::parse_netscape(raw);
        }
        Self::parse_header(raw)
    }

    fn from_pairs(pairs: Vec<(String, String)>, source: &str) -> Result<Self, XhlError> {
        let mut auth_token = None;
        let mut ct0 = None;
        let mut twid = None;
        let mut extras = Vec::new();

        for (name, value) in pairs {
            match name.trim() {
                "auth_token" => auth_token = Some(value),
                "ct0" => ct0 = Some(value),
                "twid" => twid = Some(value),
                other => extras.push((other.to_owned(), value)),
            }
        }

        let missing: Vec<&str> = [("auth_token", auth_token.is_none()), ("ct0", ct0.is_none())]
            .into_iter()
            .filter_map(|(n, missing)| missing.then_some(n))
            .collect();

        if !missing.is_empty() {
            return Err(XhlError::Invalid(format!(
                "{} tidak memuat cookie wajib: {}",
                source,
                missing.join(", ")
            )));
        }

        Ok(Self {
            auth_token: auth_token.expect("diperiksa di atas"),
            ct0: ct0.expect("diperiksa di atas"),
            twid,
            extras,
        })
    }
}

fn parse_pairs(raw: &str) -> Vec<(String, String)> {
    raw.split(';')
        .filter_map(|part| {
            let (k, v) = part.split_once('=')?;
            let k = k.trim();
            if k.is_empty() {
                return None;
            }
            Some((k.to_owned(), v.trim().to_owned()))
        })
        .collect()
}

/// Status sesi hasil health-check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Ok {
        screen_name: String,
        user_id: Option<String>,
    },
    Expired {
        detail: String,
    },
}

impl SessionStatus {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    /// Terjemahkan respons `account/settings.json` menjadi status.
    ///
    /// Dipisahkan dari I/O agar bisa diuji offline terhadap body respons nyata.
    pub fn interpret(status: http_status::Status, body: &str) -> Self {
        if status.is_auth_failure() {
            return Self::Expired {
                detail: format!("HTTP {}", status.code()),
            };
        }
        let parsed: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => {
                return Self::Expired {
                    detail: format!("respons tidak dapat dibaca: {e}"),
                }
            }
        };

        // X membalas error lewat array `errors` walau status 200.
        if let Some(errors) = parsed.get("errors").and_then(|e| e.as_array()) {
            if !errors.is_empty() {
                let msg = errors
                    .first()
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("tidak diketahui");
                return Self::Expired {
                    detail: msg.to_owned(),
                };
            }
        }

        // Bentuk 1: `account/multi/list.json` → `{"users":[{"screen_name":..,"user_id":..}]}`.
        if let Some(users) = parsed.get("users").and_then(|u| u.as_array()) {
            if let Some(first) = users.first() {
                if let Some(name) = first.get("screen_name").and_then(|s| s.as_str()) {
                    return Self::Ok {
                        screen_name: name.to_owned(),
                        user_id: first.get("user_id").map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| v.to_string())
                        }),
                    };
                }
            }
            return Self::Expired {
                detail: "daftar akun kosong pada respons".into(),
            };
        }

        // Bentuk 2: respons datar dengan `screen_name` di akar.
        match parsed.get("screen_name").and_then(|s| s.as_str()) {
            Some(name) => Self::Ok {
                screen_name: name.to_owned(),
                user_id: parsed.get("user_id").map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                }),
            },
            None => Self::Expired {
                detail: "respons tidak memuat screen_name".into(),
            },
        }
    }
}

/// Enum status HTTP minimal, supaya `session` tidak bergantung pada tipe transport.
pub mod http_status {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Status(u16);

    impl Status {
        pub fn new(code: u16) -> Self {
            Self(code)
        }

        pub fn code(self) -> u16 {
            self.0
        }

        /// 401/403 = cookie tidak berlaku lagi (bukan sekadar rate limit).
        pub fn is_auth_failure(self) -> bool {
            matches!(self.0, 401 | 403)
        }

        pub fn is_success(self) -> bool {
            (200..300).contains(&self.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::http_status::Status;
    use super::*;

    #[test]
    fn debug_meredaksi_nilai_cookie() {
        let c = Cookies {
            auth_token: "RAHASIA_AUTH".into(),
            ct0: "RAHASIA_CT0".into(),
            twid: Some("RAHASIA_TWID".into()),
            extras: vec![("guest_id".into(), "RAHASIA_GUEST".into())],
        };
        let dumped = format!("{c:?}");
        assert!(
            !dumped.contains("RAHASIA"),
            "cookie bocor di Debug: {dumped}"
        );
        assert!(dumped.contains("redacted"));
    }

    #[test]
    fn parse_string_mentah() {
        let c = Cookies::parse_header("auth_token=AAA; ct0=BBB; guest_id=v1%3A123").unwrap();
        assert_eq!(c.auth_token, "AAA");
        assert_eq!(c.csrf_token(), "BBB");
        assert_eq!(
            c.extras,
            vec![("guest_id".to_owned(), "v1%3A123".to_owned())]
        );
        assert!(c.as_cookie_header().contains("auth_token=AAA"));
    }

    #[test]
    fn parse_json_array_dan_objek() {
        let arr = r#"[{"name":"auth_token","value":"A"},{"name":"ct0","value":"C"}]"#;
        assert_eq!(Cookies::parse_json(arr).unwrap().ct0, "C");

        let obj = r#"{"auth_token":"A","ct0":"C","twid":"u=1"}"#;
        let c = Cookies::parse_json(obj).unwrap();
        assert_eq!(c.twid.as_deref(), Some("u=1"));
    }

    #[test]
    fn parse_netscape() {
        let raw = "# Netscape HTTP Cookie File\n\
.x.com\tTRUE\t/\tTRUE\t1800000000\tauth_token\tAAA\n\
.x.com\tTRUE\t/\tTRUE\t1800000000\tct0\tBBB\n";
        let c = Cookies::parse_netscape(raw).unwrap();
        assert_eq!(c.auth_token, "AAA");
        assert_eq!(c.ct0, "BBB");
    }

    #[test]
    fn cookie_tanpa_ct0_ditolak() {
        let err = Cookies::parse_header("auth_token=AAA").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ct0"), "{msg}");
        assert!(!msg.contains("AAA"), "nilai cookie bocor di pesan error");
    }

    #[test]
    fn parse_auto_mendeteksi_format() {
        assert!(Cookies::parse_auto("auth_token=A; ct0=C").is_ok());
        assert!(Cookies::parse_auto(r#"{"auth_token":"A","ct0":"C"}"#).is_ok());
    }

    #[test]
    fn interpret_ok_dari_respons_nyata() {
        let body = r#"{"screen_name":"contoh","user_id":123456,"always_use_https":true}"#;
        let s = SessionStatus::interpret(Status::new(200), body);
        assert!(s.is_ok());
        match s {
            SessionStatus::Ok {
                screen_name,
                user_id,
            } => {
                assert_eq!(screen_name, "contoh");
                assert_eq!(user_id.as_deref(), Some("123456"));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn interpret_expired_dari_401_atau_errors() {
        assert!(!SessionStatus::interpret(Status::new(401), "{}").is_ok());
        let body = r#"{"errors":[{"message":"Could not authenticate you"}]}"#;
        assert!(!SessionStatus::interpret(Status::new(200), body).is_ok());
    }
}
