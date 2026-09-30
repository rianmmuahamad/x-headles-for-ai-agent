use base64::Engine;
use regex::Regex;
use scraper::{Html, Selector};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

use super::curve::Cubic;
use super::rotation::convert_rotation_to_matrix;
use crate::error::XhlError;
use crate::http::Client as HttpClient;

/// Round a number in JavaScript style (ROUND_HALF_UP)
pub fn js_round(num: f64) -> f64 {
    let decimal_part = num - num.trunc();
    if decimal_part == -0.5 {
        num.ceil()
    } else {
        num.round()
    }
}

/// Check if a number is odd
pub fn is_odd(num: i32) -> f64 {
    if num % 2 == 1 {
        -1.0
    } else {
        0.0
    }
}

/// Convert a float to a hexadecimal string
pub fn float_to_hex(x: f64) -> String {
    if x == 0.0 {
        return "0".to_string();
    }

    let mut result = String::new();
    let mut quotient = x.floor() as i64;
    let mut fraction = x - quotient as f64;

    let parse_digit = |value: i64| {
        if value > 9 {
            std::char::from_u32((value as u32) + 55).unwrap()
        } else {
            std::char::from_digit(value as u32, 10).unwrap()
        }
    };

    // Convert integer part
    if quotient == 0 {
        result.push('0');
    } else {
        while quotient > 0 {
            let remainder = quotient % 16;
            quotient /= 16;
            result.insert(0, parse_digit(remainder));
        }
    }

    // Convert fraction part
    if fraction > 0.0 {
        result.push('.');
        while fraction > 0.0 {
            fraction *= 16.0;
            let integer = fraction.floor() as i64;
            fraction -= integer as f64;
            result.push(parse_digit(integer));
            if result.len() > 20 {
                break;
            }
        }
    }

    result
}

pub fn base64_encode<T: AsRef<[u8]>>(data: T) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn base64_decode<T: AsRef<[u8]>>(input: T) -> Result<Vec<u8>, base64::DecodeError> {
    let bytes = input.as_ref();
    if let Ok(res) = base64::engine::general_purpose::STANDARD.decode(bytes) {
        return Ok(res);
    }
    base64::engine::general_purpose::STANDARD_NO_PAD.decode(bytes)
}

pub fn interpolate(from_list: &[f64], to_list: &[f64], f: f64) -> Result<Vec<f64>, XhlError> {
    if from_list.len() != to_list.len() {
        return Err(XhlError::Internal(
            "interpolate: panjang list tidak sama".into(),
        ));
    }
    let mut out = Vec::with_capacity(from_list.len());
    for i in 0..from_list.len() {
        out.push(interpolate_num(from_list[i], to_list[i], f));
    }
    Ok(out)
}

pub fn interpolate_num(from_val: f64, to_val: f64, f: f64) -> f64 {
    from_val * (1.0 - f) + to_val * f
}

/// State hasil bootstrap dari halaman X + bundle ondemand.
///
/// Dapat diserialisasi karena nilainya hanya bergantung pada bundle X, bukan
/// pada request — sehingga bisa dipakai ulang antar proses (lihat
/// `TransactionSigner::with_cache`).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TransactionState {
    pub key_bytes: Vec<u8>,
    pub animation_key: String,
    pub default_keyword: String,
    pub additional_random_number: u8,
}

pub struct TransactionSigner {
    state: RwLock<Option<TransactionState>>,
    client: HttpClient,
    /// Cookie sesi + profil untuk bootstrap.
    ///
    /// Halaman `x.com` versi anonim **tidak** memuat `ondemand.s`; hanya versi
    /// logged-in yang memuatnya. Karena itu bootstrap wajib memakai cookie.
    cookies: Option<(
        crate::session::Cookies,
        &'static crate::http::headers::HttpProfile,
    )>,
    /// Cache persisten state, agar proses berikutnya tidak perlu 1–2 fetch lagi.
    cache: Option<(crate::store::StoreHandle, String)>,
}

impl TransactionSigner {
    pub fn new(client: HttpClient) -> Self {
        Self {
            state: RwLock::new(None),
            client,
            cookies: None,
            cache: None,
        }
    }

    /// Signer yang bootstrap-nya memakai cookie sesi (wajib untuk X saat ini).
    pub fn with_session(
        client: HttpClient,
        cookies: crate::session::Cookies,
        profile: &'static crate::http::headers::HttpProfile,
    ) -> Self {
        Self {
            state: RwLock::new(None),
            client,
            cookies: Some((cookies, profile)),
            cache: None,
        }
    }

    /// Tambahkan cache persisten (store + nama akun).
    ///
    /// State yang sudah tersimpan dipakai langsung; bootstrap hanya dilakukan
    /// bila cache kosong atau state-nya ditolak X.
    pub fn with_cache(
        mut self,
        store: crate::store::StoreHandle,
        account: impl Into<String>,
    ) -> Self {
        self.cache = Some((store, account.into()));
        self
    }

    /// Versi yang menerima `Option<StoreHandle>`; `None` berarti tanpa cache.
    pub fn with_cache_store(
        self,
        store: Option<crate::store::StoreHandle>,
        account: impl Into<String>,
    ) -> Self {
        match store {
            Some(s) => self.with_cache(s, account),
            None => self,
        }
    }

    /// Muat state dari cache bila ada. Gagal membaca cache bukan kesalahan fatal.
    async fn load_cached(&self) -> Option<TransactionState> {
        let (store, account) = self.cache.as_ref()?;
        let account = account.clone();
        let raw = store.run(move |s| s.antibot_state(&account)).await.ok()??;
        match serde_json::from_str::<TransactionState>(&raw) {
            Ok(st) => {
                tracing::debug!("signer: memakai state dari cache");
                Some(st)
            }
            Err(e) => {
                tracing::debug!(error = %e, "signer: cache tidak dapat dibaca, bootstrap ulang");
                None
            }
        }
    }

    /// Simpan state ke cache. Kegagalan tidak boleh menggagalkan request.
    async fn save_cached(&self, state: &TransactionState) {
        let Some((store, account)) = self.cache.as_ref() else {
            return;
        };
        let account = account.clone();
        let Ok(json) = serde_json::to_string(state) else {
            return;
        };
        if let Err(e) = store
            .run(move |s| s.save_antibot_state(&account, &json, "bootstrap"))
            .await
        {
            tracing::debug!(error = %e, "signer: gagal menyimpan cache");
        }
    }

    /// Header untuk mengambil halaman/bundle statis.
    ///
    /// **Cookie saja.** Header `authorization: Bearer` milik endpoint
    /// `x.com/i/api`; bila ikut dikirim ke permintaan halaman HTML, Cloudflare
    /// X menolak koneksi. Halaman tanpa header itu terkirim lengkap (304 KB) dan
    /// memuat `ondemand.s`, sedangkan dengan header tersebut gagal.
    fn page_headers(&self) -> crate::http::client::HeaderMap {
        match &self.cookies {
            Some((cookies, profile)) => Self::page_headers_for(cookies, profile),
            None => Vec::new(),
        }
    }

    /// Versi bebas dari state, dipakai juga oleh discovery queryId.
    pub fn page_headers_for(
        cookies: &crate::session::Cookies,
        profile: &'static crate::http::headers::HttpProfile,
    ) -> crate::http::client::HeaderMap {
        // Override profil berlaku juga di sini: halaman bootstrap dan discovery
        // adalah permintaan yang paling mudah ditandai bot, jadi identitasnya
        // harus sama dengan permintaan GraphQL.
        crate::http::headers::apply_profile_overrides(vec![
            ("user-agent".to_string(), profile.user_agent.to_string()),
            ("cookie".to_string(), cookies.as_cookie_header()),
            (
                "accept".to_string(),
                "text/html,application/xhtml+xml".to_string(),
            ),
            ("accept-language".to_string(), "en-US,en;q=0.9".to_string()),
        ])
    }

    pub fn is_ready(&self) -> bool {
        self.state.try_read().map(|g| g.is_some()).unwrap_or(false)
    }

    pub async fn sign(&self, method: &str, path: &str) -> Result<String, XhlError> {
        // Fast path: jika sudah siap, langsung tanda tangani
        {
            let r = self.state.read().await;
            if let Some(st) = r.as_ref() {
                return Ok(Self::generate_from_state(st, method, path));
            }
        }

        // Slow path: cache persisten, lalu bootstrap.
        let mut w = self.state.write().await;
        if let Some(st) = w.as_ref() {
            return Ok(Self::generate_from_state(st, method, path));
        }

        if let Some(cached) = self.load_cached().await {
            let res = Self::generate_from_state(&cached, method, path);
            *w = Some(cached);
            return Ok(res);
        }

        let new_state = self.bootstrap().await?;
        let res = Self::generate_from_state(&new_state, method, path);
        self.save_cached(&new_state).await;
        *w = Some(new_state);
        Ok(res)
    }

    pub async fn refresh(&self) -> Result<(), XhlError> {
        let new_state = self.bootstrap().await?;
        self.save_cached(&new_state).await;
        let mut w = self.state.write().await;
        *w = Some(new_state);
        Ok(())
    }

    pub fn generate_from_state(state: &TransactionState, method: &str, path: &str) -> String {
        let time_now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(1682924400) as u32;

        let time_now_bytes = [
            (time_now & 0xFF) as u8,
            ((time_now >> 8) & 0xFF) as u8,
            ((time_now >> 16) & 0xFF) as u8,
            ((time_now >> 24) & 0xFF) as u8,
        ];

        let hash_input = format!(
            "{}!{}!{}{}{}",
            method, path, time_now, state.default_keyword, state.animation_key
        );

        let mut hasher = Sha256::new();
        hasher.update(hash_input.as_bytes());
        let hash_result = hasher.finalize();
        let hash_bytes: Vec<u8> = hash_result[..16].to_vec();

        let random_num = rand::random::<u8>();

        let mut bytes_arr =
            Vec::with_capacity(state.key_bytes.len() + time_now_bytes.len() + hash_bytes.len() + 1);
        bytes_arr.extend_from_slice(&state.key_bytes);
        bytes_arr.extend_from_slice(&time_now_bytes);
        bytes_arr.extend_from_slice(&hash_bytes);
        bytes_arr.push(state.additional_random_number);

        let mut out = vec![random_num];
        out.extend(bytes_arr.iter().map(|&b| b ^ random_num));

        let encoded = base64_encode(&out);
        encoded.trim_end_matches('=').to_string()
    }

    /// Ambil halaman home yang memuat `ondemand.s`.
    ///
    /// `https://x.com/home` lebih kecil (~17 KB) dan memuat keduanya; fallback
    /// ke `https://x.com` bila strukturnya berubah.
    async fn fetch_home_page(
        &self,
        page_headers: &crate::http::client::HeaderMap,
    ) -> Result<String, XhlError> {
        const CANDIDATES: &[&str] = &["https://x.com/home", "https://x.com"];
        let marker = Regex::new(r#",(\d+):["']ondemand\.s["']"#).unwrap();

        let mut last: Option<XhlError> = None;
        for url in CANDIDATES {
            match self.client.get(url, page_headers).await {
                Ok(resp) => {
                    if marker.is_match(&resp.body) {
                        tracing::debug!(
                            url,
                            bytes = resp.body.len(),
                            "bootstrap: halaman memuat ondemand.s"
                        );
                        return Ok(resp.body);
                    }
                    tracing::debug!(
                        url,
                        bytes = resp.body.len(),
                        "bootstrap: halaman tanpa ondemand.s"
                    );
                }
                Err(e) => {
                    tracing::debug!(url, error = %e, "bootstrap: halaman gagal diambil");
                    last = Some(e);
                }
            }
        }

        Err(last.unwrap_or_else(|| {
            XhlError::AntiBotStateUnavailable(format!(
                "tidak ada halaman X (dari {}) yang memuat ondemand.s",
                CANDIDATES.join(", ")
            ))
        }))
    }

    async fn bootstrap(&self) -> Result<TransactionState, XhlError> {
        // Halaman yang diambil tanpa cookie tidak memuat `ondemand.s` pada X
        // saat ini; kirim header sesi bila tersedia.
        let page_headers = self.page_headers();
        let html_text = self.fetch_home_page(&page_headers).await?;

        let (row_index, key_bytes_indices) =
            Self::get_indices(&html_text, &self.client, &page_headers).await?;

        let home_page = Html::parse_document(&html_text);
        // Periksa key site verification
        let key_str = Self::get_key(&home_page)?;
        let key_bytes = base64_decode(&key_str).map_err(|e| {
            XhlError::AntiBotStateUnavailable(format!("decode site verification key: {e}"))
        })?;

        let animation_key =
            Self::get_animation_key(&key_bytes, &home_page, row_index, &key_bytes_indices)?;
        Ok(TransactionState {
            key_bytes,
            animation_key,
            default_keyword: "obfiowerehiring".to_string(),
            additional_random_number: 3,
        })
    }

    fn get_key(page: &Html) -> Result<String, XhlError> {
        let selector = Selector::parse("[name='twitter-site-verification']").map_err(|_| {
            XhlError::AntiBotStateUnavailable(
                "gagal membuat selector twitter-site-verification".into(),
            )
        })?;
        let element = page.select(&selector).next().ok_or_else(|| {
            XhlError::AntiBotStateUnavailable(
                "tidak dapat menemukan twitter-site-verification di HTML x.com".into(),
            )
        })?;
        let key = element.value().attr("content").ok_or_else(|| {
            XhlError::AntiBotStateUnavailable(
                "missing content attribute di twitter-site-verification".into(),
            )
        })?;
        Ok(key.to_string())
    }

    async fn get_indices(
        html: &str,
        client: &HttpClient,
        page_headers: &crate::http::client::HeaderMap,
    ) -> Result<(usize, Vec<usize>), XhlError> {
        let on_demand_file_regex = Regex::new(r#",(\d+):["']ondemand\.s["']"#).unwrap();
        let on_demand_file_index = on_demand_file_regex
            .captures(html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| {
                XhlError::AntiBotStateUnavailable("tidak dapat menemukan indeks ondemand.s".into())
            })?;

        let regex_name = Regex::new(&format!(r#"{}:"([0-9a-f]+)""#, on_demand_file_index)).unwrap();
        let on_demand_file_name = regex_name
            .captures(html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| {
                XhlError::AntiBotStateUnavailable(
                    "tidak dapat menemukan nama file ondemand.s".into(),
                )
            })?;

        let on_demand_file_url = format!(
            "https://abs.twimg.com/responsive-web/client-web/ondemand.s.{}a.js",
            on_demand_file_name
        );

        let resp = client.get(&on_demand_file_url, page_headers).await?;
        let on_demand_content = resp.body;

        let indices_regex = Regex::new(r#"(\(\w{1}\[(\d{1,2})\],\s*16\))+"#).unwrap();
        let mut key_byte_indices = Vec::new();
        for captures in indices_regex.captures_iter(&on_demand_content) {
            if let Some(index_match) = captures.get(2) {
                if let Ok(index) = index_match.as_str().parse::<usize>() {
                    key_byte_indices.push(index);
                }
            }
        }

        if key_byte_indices.is_empty() {
            return Err(XhlError::AntiBotStateUnavailable(
                "tidak dapat mengekstrak KEY_BYTE indices dari ondemand.s".into(),
            ));
        }

        Ok((key_byte_indices[0], key_byte_indices[1..].to_vec()))
    }

    fn get_animation_key(
        key_bytes: &[u8],
        page: &Html,
        row_index: usize,
        key_bytes_indices: &[usize],
    ) -> Result<String, XhlError> {
        let total_time = 4096.0;
        if key_bytes.is_empty() || row_index >= key_bytes.len() {
            return Err(XhlError::AntiBotStateUnavailable(
                "key_bytes kosong atau row_index out of bounds".into(),
            ));
        }
        let row_index_value = (key_bytes[row_index] % 16) as usize;

        let frame_time = key_bytes_indices
            .iter()
            .map(|&index| {
                if index < key_bytes.len() {
                    (key_bytes[index] % 16) as f64
                } else {
                    1.0
                }
            })
            .fold(1.0, |acc, val| acc * val);

        let frame_time = js_round(frame_time / 10.0) * 10.0;
        let arr = Self::get_2d_array(key_bytes, page)?;

        if row_index_value >= arr.len() {
            return Err(XhlError::AntiBotStateUnavailable(
                "row_index_value out of bounds di 2D array".into(),
            ));
        }

        let frame_row = &arr[row_index_value];
        let target_time = frame_time / total_time;
        let animation_key = Self::animate(frame_row, target_time)?;
        Ok(animation_key)
    }

    fn get_2d_array(key_bytes: &[u8], page: &Html) -> Result<Vec<Vec<i32>>, XhlError> {
        let selector = Selector::parse("[id^='loading-x-anim']").map_err(|_| {
            XhlError::AntiBotStateUnavailable("selector loading-x-anim invalid".into())
        })?;
        let frames: Vec<_> = page.select(&selector).collect();
        if frames.is_empty() {
            return Err(XhlError::AntiBotStateUnavailable(
                "elemen loading-x-anim tidak ditemukan di HTML".into(),
            ));
        }

        if key_bytes.len() <= 5 {
            return Err(XhlError::AntiBotStateUnavailable(
                "key_bytes terlalu pendek (<6 byte)".into(),
            ));
        }

        let frame_index = (key_bytes[5] % 4) as usize;
        if frame_index >= frames.len() {
            return Err(XhlError::AntiBotStateUnavailable(
                "frame_index out of bounds".into(),
            ));
        }

        let frame = frames[frame_index];
        let mut outer_children = frame.children();
        let first_child = outer_children.next().ok_or_else(|| {
            XhlError::AntiBotStateUnavailable("frame tidak memiliki anak pertama".into())
        })?;
        let first_child = scraper::ElementRef::wrap(first_child)
            .ok_or_else(|| XhlError::AntiBotStateUnavailable("first_child bukan element".into()))?;

        let mut inner_children = first_child.children();
        let path_node = inner_children.nth(1).ok_or_else(|| {
            XhlError::AntiBotStateUnavailable("tidak ada anak kedua di inner group".into())
        })?;
        let path_elem = scraper::ElementRef::wrap(path_node)
            .ok_or_else(|| XhlError::AntiBotStateUnavailable("path_node bukan element".into()))?;

        let d_attr = path_elem.value().attr("d").ok_or_else(|| {
            XhlError::AntiBotStateUnavailable("missing 'd' attribute di path loading-x-anim".into())
        })?;

        let d_content = d_attr.get(9..).ok_or_else(|| {
            XhlError::AntiBotStateUnavailable("path 'd' data terlalu pendek".into())
        })?;

        let segments = d_content.split('C');
        let mut result = Vec::new();
        for segment in segments {
            let numbers: Vec<i32> = segment
                .replace(|c: char| !c.is_ascii_digit() && c != '-', " ")
                .split_whitespace()
                .filter_map(|s| s.parse::<i32>().ok())
                .collect();
            result.push(numbers);
        }

        Ok(result)
    }

    fn solve(value: f64, min_val: f64, max_val: f64, rounding: bool) -> f64 {
        let result = value * (max_val - min_val) / 255.0 + min_val;
        if rounding {
            result.floor()
        } else {
            (result * 100.0).round() / 100.0
        }
    }

    fn animate(frames: &[i32], target_time: f64) -> Result<String, XhlError> {
        if frames.len() < 7 {
            return Err(XhlError::AntiBotStateUnavailable(
                "frames array terlalu pendek untuk animasi (<7)".into(),
            ));
        }

        let from_color: Vec<f64> = frames[..3]
            .iter()
            .map(|&i| i as f64)
            .chain(std::iter::once(1.0))
            .collect();
        let to_color: Vec<f64> = frames[3..6]
            .iter()
            .map(|&i| i as f64)
            .chain(std::iter::once(1.0))
            .collect();
        let from_rotation = vec![0.0];
        let to_rotation = vec![Self::solve(frames[6] as f64, 60.0, 360.0, true)];

        let curves: Vec<f64> = frames[7..]
            .iter()
            .enumerate()
            .map(|(i, &val)| Self::solve(val as f64, is_odd(i as i32), 1.0, false))
            .collect();

        let cubic = Cubic::new(curves);
        let val = cubic.get_value(target_time);

        let color = interpolate(&from_color, &to_color, val)?;
        let color: Vec<f64> = color.iter().map(|&v| v.clamp(0.0, 255.0)).collect();

        let rotation = interpolate(&from_rotation, &to_rotation, val)?;
        let matrix = convert_rotation_to_matrix(rotation[0]);

        let mut str_arr = Vec::new();

        // Add color values as hex
        for value in &color[..color.len().saturating_sub(1)] {
            str_arr.push(format!("{:x}", value.round() as i32));
        }

        // Add matrix values as hex
        for value in matrix {
            let rounded = (value * 100.0).round() / 100.0;
            let abs_value = rounded.abs();
            let hex_value = float_to_hex(abs_value);

            if hex_value.starts_with('.') {
                str_arr.push(format!("0{}", hex_value.to_lowercase()));
            } else if hex_value.is_empty() {
                str_arr.push("0".to_string());
            } else {
                str_arr.push(hex_value.to_lowercase());
            }
        }

        str_arr.push("0".to_string());
        str_arr.push("0".to_string());

        let animation_key = str_arr.join("");
        Ok(animation_key.replace(['.', '-'], ""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_odd() {
        assert_eq!(is_odd(1), -1.0);
        assert_eq!(is_odd(2), 0.0);
        assert_eq!(is_odd(3), -1.0);
        assert_eq!(is_odd(4), 0.0);
    }

    #[test]
    fn test_js_round() {
        assert_eq!(js_round(0.0), 0.0);
        assert_eq!(js_round(0.4), 0.0);
        assert_eq!(js_round(0.5), 1.0);
        assert_eq!(js_round(0.6), 1.0);
        assert_eq!(js_round(1.5), 2.0);

        assert_eq!(js_round(-0.0), 0.0);
        assert_eq!(js_round(-0.4), 0.0);
        assert_eq!(js_round(-0.5), -0.0);
        assert_eq!(js_round(-0.6), -1.0);
        assert_eq!(js_round(-1.5), -1.0);
    }

    #[test]
    fn test_float_to_hex() {
        assert_eq!(float_to_hex(10.0), "A");
        assert_eq!(float_to_hex(16.0), "10");
        assert_eq!(float_to_hex(0.5), "0.8");
    }

    #[test]
    fn test_interpolate() {
        let from = vec![0.0, 10.0, 20.0];
        let to = vec![100.0, 110.0, 120.0];
        let result = interpolate(&from, &to, 0.5).unwrap();
        assert_eq!(result, vec![50.0, 60.0, 70.0]);

        let diff_len = vec![1.0];
        assert!(interpolate(&from, &diff_len, 0.5).is_err());
    }

    #[test]
    fn test_generate_transaction_id_structure() {
        let state = TransactionState {
            key_bytes: vec![42; 48],
            animation_key: "0123456789abcdef".to_string(),
            default_keyword: "obfiowerehiring".to_string(),
            additional_random_number: 3,
        };

        let tid = TransactionSigner::generate_from_state(
            &state,
            "POST",
            "/i/api/graphql/xyz/CreateTweet",
        );
        assert!(!tid.is_empty());

        // Base64 decode to verify XOR structure
        let decoded = base64_decode(&tid).expect("hasil harus base64 valid");
        assert!(decoded.len() > 1);

        let random_key = decoded[0];
        let restored_bytes: Vec<u8> = decoded[1..].iter().map(|b| b ^ random_key).collect();

        // 48 bytes key_bytes + 4 bytes time + 16 bytes hash + 1 byte (3) = 69 bytes
        assert_eq!(restored_bytes.len(), 48 + 4 + 16 + 1);
        assert_eq!(&restored_bytes[..48], &state.key_bytes[..]);
        assert_eq!(*restored_bytes.last().unwrap(), 3);
    }
}
