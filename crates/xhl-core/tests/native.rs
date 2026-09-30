//! Uji perilaku inti C++ pada batasnya: tepat di ambang, tepat di luar, dan
//! pada masukan yang harus ditolak. Setiap uji di sini gagal bila aturan yang
//! diport dari `coherence.py` berubah perilaku.

use xhl_core::native::coherence::{
    apply_coherence, check_profile, drop_incoherent, headers_from_profile,
};
use xhl_core::native::profile::ProfileHandle;
use xhl_core::native::util::{draw_frame, percentile, text_width};
use xhl_core::native::NativeConfig;

fn rules(json: &str, os: &str) -> Vec<String> {
    check_profile(json, os)
        .expect("validasi koherensi gagal dijalankan")
        .into_iter()
        .map(|v| v.rule)
        .collect()
}

// --- Fase 4: percentile, text_width, gambar ---

#[test]
fn percentile_interpolasi_dan_batas() {
    let samples: Vec<u64> = (1..=100).collect();
    assert!((percentile(&samples, 0.5).unwrap() - 50.5).abs() < 1e-9);
    assert!((percentile(&samples, 0.0).unwrap() - 1.0).abs() < 1e-9);
    assert!((percentile(&samples, 1.0).unwrap() - 100.0).abs() < 1e-9);
    // Satu sampel: nilai itu sendiri.
    assert!((percentile(&[7], 0.9).unwrap() - 7.0).abs() < 1e-9);
    // Sampel tidak terurut tetap benar.
    assert!((percentile(&[3, 1, 2], 1.0).unwrap() - 3.0).abs() < 1e-9);
    // Kosong ditolak; p di luar [0,1] ditolak.
    assert!(percentile(&[], 0.5).is_err());
    assert!(percentile(&samples, 1.5).is_err());
}

#[test]
fn text_width_menghitung_glyph_dan_rune_lain() {
    // Setiap glyph = 5 kolom + 1 jarak.
    assert_eq!(text_width("ABC").unwrap(), 18);
    assert_eq!(
        text_width("a").unwrap(),
        6,
        "huruf kecil memakai glyph kapital"
    );
    // Rune non-ASCII tidak punya glyph: 1 kolom, bukan 6.
    assert_eq!(text_width("é").unwrap(), 1);
    assert_eq!(text_width("").unwrap(), 0);
}

#[test]
fn draw_frame_menulis_bmp_24bit_non_kosong() {
    let out = std::env::temp_dir().join("xhl-test-frame.bmp");
    draw_frame("xhl", &out, 1).unwrap();
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.len() > 54, "BMP harus memuat header + piksel");
    assert_eq!(&bytes[0..2], b"BM", "tanda berkas BMP");
    // Offset +28: kedalaman bit.
    assert_eq!(u16::from_le_bytes([bytes[28], bytes[29]]), 24);
    // Lebih dari nol piksel hitam: bukti glyph benar-benar digambar.
    let black = bytes[54..].iter().filter(|b| **b == 0).count();
    assert!(black > 0, "teks harus menghasilkan piksel hitam");
    let _ = std::fs::remove_file(&out);
}

// --- Fase 3: skema profil ---

#[test]
fn voices_separuh_ditolak_dengan_indeks_dan_field() {
    let err = ProfileHandle::validate(r#"{"voices":[{"lang":"en","name":"A"}]}"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("voices[0]"), "{err}");
    assert!(err.contains("voiceUri"), "{err}");
}

#[test]
fn avail_width_tanpa_height_ditolak() {
    let err = ProfileHandle::validate(r#"{"screen.availWidth":1920}"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("screen.availHeight"), "{err}");
}

#[test]
fn shader_precision_menuntut_tiga_field_dan_kunci_berpasangan() {
    // Kunci bukan "<int>,<int>".
    let err = ProfileHandle::validate(
        r#"{"webGl:shaderPrecisionFormats":{"abc":{"rangeMin":1,"rangeMax":2,"precision":3}}}"#,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("abc"), "{err}");

    // Field hilang.
    let err = ProfileHandle::validate(
        r#"{"webGl:shaderPrecisionFormats":{"1,2":{"rangeMin":1,"rangeMax":2}}}"#,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("precision"), "{err}");

    // Kunci bergaya "1>2" juga diterima.
    ProfileHandle::validate(
        r#"{"webGl:shaderPrecisionFormats":{"1>2":{"rangeMin":1,"rangeMax":2,"precision":3}}}"#,
    )
    .unwrap();
}

#[test]
fn uint_negatif_ditolak_bukan_dipotong() {
    let err = ProfileHandle::validate(r#"{"screen.width":-100}"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("screen.width"), "{err}");
    assert!(err.contains("negatif"), "{err}");
}

#[test]
fn profil_bolak_balik_tetap_sah() {
    let src = r#"{"navigator.userAgent":"UA","screen.width":1920,"screen.height":1080}"#;
    let h = ProfileHandle::from_json(src).unwrap();
    let dumped = h.dump().unwrap();
    ProfileHandle::validate(&dumped).expect("hasil dump harus tetap sah");
    drop(h);
    assert!(!ProfileHandle::is_loaded());
}

// --- Fase 6: koherensi (aturan dari coherence.py) ---

#[test]
fn apple_silicon_cores_dan_perbaikan_naik() {
    let json = r#"{"webGl:renderer":"Apple M1, or similar","navigator.hardwareConcurrency":9}"#;
    assert!(rules(json, "mac").contains(&"apple-silicon-cores".to_string()));
    let fixed = apply_coherence(json, "mac").unwrap();
    // 9 bukan bagian Apple: naik ke yang berikutnya yang sah, yaitu 10.
    assert!(
        fixed.json.contains("\"navigator.hardwareConcurrency\": 10"),
        "{}",
        fixed.json
    );
}

#[test]
fn gpu_angle_di_mac_dilaporkan_tidak_diperbaiki() {
    let json = r#"{"webGl:renderer":"ANGLE (NVIDIA)"}"#;
    let report = apply_coherence(json, "mac").unwrap();
    // Tidak ada nilai benar yang dapat ditentukan: tetap dilaporkan.
    assert!(report.violations.iter().any(|v| v.rule == "gpu-matches-os"));
}

#[test]
fn color_depth_diperbaiki_ke_30_untuk_apple_silicon() {
    let json = r#"{"webGl:renderer":"Apple M1, or similar","screen.colorDepth":24}"#;
    assert!(rules(json, "mac").contains(&"color-depth".to_string()));
    let fixed = apply_coherence(json, "mac").unwrap();
    assert!(
        fixed.json.contains("\"screen.colorDepth\": 30"),
        "{}",
        fixed.json
    );
    // pixelDepth mengikuti colorDepth.
    assert!(
        fixed.json.contains("\"screen.pixelDepth\": 30"),
        "{}",
        fixed.json
    );
}

#[test]
fn touch_points_berlebihan_diperbaiki_ke_nol() {
    let json = r#"{"navigator.maxTouchPoints":256}"#;
    assert!(rules(json, "win").contains(&"touch-points".to_string()));
    let fixed = apply_coherence(json, "win").unwrap();
    assert!(
        fixed.json.contains("\"navigator.maxTouchPoints\": 0"),
        "{}",
        fixed.json
    );
}

#[test]
fn device_pixel_ratio_digeser_ke_langkah_valid_terdekat() {
    // 1.09 -> 1 (Linux), bukan 1.25.
    let fixed = apply_coherence(r#"{"window.devicePixelRatio":1.09}"#, "lin").unwrap();
    assert!(
        fixed.json.contains("\"window.devicePixelRatio\": 1.0"),
        "{}",
        fixed.json
    );
    // 1.818 -> 1.75 (Windows).
    let fixed = apply_coherence(r#"{"window.devicePixelRatio":1.818}"#, "win").unwrap();
    assert!(fixed.json.contains("1.75"), "{}", fixed.json);
}

#[test]
fn avail_bounds_lebih_besar_dipotong() {
    let json = r#"{"screen.width":1920,"screen.height":1080,
                   "screen.availWidth":2000,"screen.availHeight":1080}"#;
    assert!(rules(json, "lin").contains(&"avail-bounds".to_string()));
    let fixed = apply_coherence(json, "lin").unwrap();
    assert!(
        fixed.json.contains("\"screen.availWidth\": 1920"),
        "{}",
        fixed.json
    );
}

#[test]
fn arch_agreement_ua_x86_dengan_oscpu_armv() {
    let json = r#"{"navigator.userAgent":"Mozilla/5.0 (X11; Linux x86_64)",
                   "navigator.oscpu":"Linux armv8l"}"#;
    assert!(rules(json, "lin").contains(&"arch-agreement".to_string()));
}

#[test]
fn screen_portrait_dan_terlalu_kecil_dilaporkan() {
    assert!(
        rules(r#"{"screen.width":1440,"screen.height":2560}"#, "lin")
            .contains(&"screen-shape".to_string())
    );
    assert!(rules(r#"{"screen.width":800,"screen.height":600}"#, "lin")
        .contains(&"screen-shape".to_string()));
}

#[test]
fn window_chrome_kurang_dari_86_diperbaiki() {
    let json = r#"{"window.innerHeight":717,"window.outerHeight":745}"#;
    assert!(rules(json, "lin").contains(&"window-chrome".to_string()));
    let fixed = apply_coherence(json, "lin").unwrap();
    // Tanpa avail: jendela diperbesar agar selisihnya = 86.
    let v: serde_json::Value = serde_json::from_str(&fixed.json).unwrap();
    let inner = v["window.innerHeight"].as_i64().unwrap();
    let outer = v["window.outerHeight"].as_i64().unwrap();
    assert_eq!(outer - inner, 86, "{}", fixed.json);
}

#[test]
fn intel_mac_gpu_dengan_panel_apple_silicon_dilaporkan() {
    let json = r#"{"webGl:renderer":"Intel(R) HD Graphics 400",
                   "screen.width":1512,"screen.height":982}"#;
    assert!(rules(json, "mac").contains(&"intel-mac-hardware".to_string()));
}

#[test]
fn drop_incoherent_membuang_gpu_asing() {
    let json = r#"{"webGl:renderer":"ANGLE (NVIDIA)","webGl:vendor":"Google"}"#;
    let report = drop_incoherent(json, "mac").unwrap();
    // Nilai yang tidak dapat dipertahankan identitas dibuang, bukan diganti.
    assert!(
        !report.json.contains("NVIDIA"),
        "renderer asing harus dibuang: {}",
        report.json
    );
    assert!(
        !report.json.contains("webGl:vendor"),
        "vendor pasangannya ikut dibuang: {}",
        report.json
    );
    assert!(report.violations.iter().any(|v| v.rule == "gpu-matches-os"));
}

#[test]
fn target_os_tidak_dikenal_ditolak() {
    let err = check_profile("{}", "bsd").unwrap_err().to_string();
    assert!(err.contains("bsd"), "{err}");
}

#[test]
fn identitas_koheren_tidak_menghasilkan_pelanggaran() {
    let json = r#"{"navigator.userAgent":"Mozilla/5.0 (X11; Linux x86_64)",
                   "navigator.platform":"Linux x86_64","navigator.oscpu":"Linux x86_64",
                   "navigator.hardwareConcurrency":8,"navigator.maxTouchPoints":0,
                   "screen.width":1920,"screen.height":1080,
                   "screen.availWidth":1920,"screen.availHeight":1040,
                   "screen.colorDepth":24,"screen.pixelDepth":24,
                   "window.devicePixelRatio":1,"window.innerHeight":954,"window.outerHeight":1040}"#;
    let found = rules(json, "lin");
    assert!(found.is_empty(), "{found:?}");
}

// --- Fase 6: header ---

#[test]
fn header_override_dari_profil_dan_urutan() {
    let json = r#"{"headers.User-Agent":"UA-Profil",
                   "headers.Accept-Language":"id-ID,id;q=0.9",
                   "headers.order":["cookie","user-agent"]}"#;
    let h = headers_from_profile(json)
        .unwrap()
        .expect("profil menyumbang header");
    assert_eq!(h.user_agent.as_deref(), Some("UA-Profil"));
    assert_eq!(h.accept_language.as_deref(), Some("id-ID,id;q=0.9"));
    assert_eq!(
        h.order,
        vec!["cookie".to_string(), "user-agent".to_string()]
    );
    assert!(!h.user_agent_from_navigator);
}

#[test]
fn header_user_agent_jatuh_ke_navigator_bila_headers_absen() {
    let h = headers_from_profile(r#"{"navigator.userAgent":"UA-Nav"}"#)
        .unwrap()
        .expect("navigator.userAgent dianggap sumbangan");
    assert_eq!(h.user_agent.as_deref(), Some("UA-Nav"));
    assert!(h.user_agent_from_navigator);
}

#[test]
fn profil_tanpa_header_mengembalikan_none() {
    assert!(headers_from_profile(r#"{"screen.width":1920}"#)
        .unwrap()
        .is_none());
}

// --- Fase 2: config native ---

#[test]
fn load_error_tidak_panic_dan_config_tetap_ada() {
    // Tanpa XHL_CONFIG di lingkungan uji ini, load harus sukses (kosong).
    let cfg = NativeConfig::load().expect("load tanpa config tidak boleh gagal");
    assert!(cfg.dump().unwrap().starts_with('{'));
    // Kunci yang tidak ada -> None, bukan error.
    assert!(cfg.checked_u32("content_max_bytes").unwrap().is_none());
    // Daftar kunci dikenal tidak kosong dan memuat kunci inti.
    let keys = cfg.known_keys().unwrap();
    assert!(keys.contains(&"http_profile".to_string()));
    assert!(keys.contains(&"emulation_profile".to_string()));
}
