// Bangun inti C++ xhl (native/).
//
// Memakai `cc` (bukan cmake/bindgen) karena toolchain mesin target hanya
// memerlukan compiler C++: cmake, clang, libclang, dan ninja tidak diandaikan
// ada.

fn main() {
    let mut build = cc::Build::new();
    build
        .cpp(true) // .cpp, bukan .c: butuh string_view/optional/variant
        .std("c++17")
        .include("native")
        // JSON_NOEXCEPTION: nlohmann tidak boleh melempar, karena inti C++
        // dibangun dengan -fno-exceptions. Penguraian memakai
        // `json::accept` + `parse(..., allow_exceptions=false)`.
        .define("JSON_NOEXCEPTION", None)
        .file("native/entry.cpp")
        .file("native/config.cpp")
        .file("native/profile.cpp")
        .file("native/coherence.cpp")
        .file("native/antidetect.cpp")
        .file("native/util.cpp")
        // -fno-exceptions / -fno-rtti: hasil dilaporkan lewat kode status,
        // bukan exception; tidak ada dynamic_cast di inti ini.
        .flag_if_supported("-fno-exceptions")
        .flag_if_supported("-fno-rtti")
        .warnings(true)
        .extra_warnings(true)
        // Tautkan libstdc++ secara statis. Ini harus lewat `cpp_link_stdlib_static`,
        // bukan `cargo:rustc-link-arg=-static-libstdc++`: cc-rs juga mengemisi
        // `cargo:rustc-link-lib=stdc++` (dinamis), dan flag penautan lewat
        // rustc-link-arg tidak menggantikan permintaan dinamis itu — hasilnya
        // binary tetap menautkan libstdc++.so.6 (terverifikasi via `ldd`).
        .cpp_link_stdlib_static(true)
        .compile("xhl_native");

    println!("cargo:rerun-if-changed=native/");
}
