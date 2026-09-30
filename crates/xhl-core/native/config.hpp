// Config tervalidasi dari environment (adaptasi pola `MaskConfig.hpp` camoufox).
//
// Yang diadaptasi, dan alasannya:
//  - varian bernomor `XHL_CONFIG_1..N` **disambung sebagai teks**, lalu jatuh ke
//    `XHL_CONFIG`; menyambung (bukan menggabung objek) memungkinkan pemanggil
//    memecah satu JSON panjang melewati batas panjang env var.
//  - `std::once_flag` + `std::call_once`: config tidak berubah setelah start,
//    sedangkan accessor dipanggil di jalur panas.
//  - `nlohmann::json::accept` diperiksa lebih dulu supaya JSON tidak sah
//    terdeteksi sebelum parse, dan pesan errornya menyebut nama env.
//  - tipe salah GAGAL KERAS dengan pesan yang menyebut nama kunci, bukan
//    konversi diam-diam.

#pragma once

#include "common.hpp"

extern "C" {

// Baca `{prefix}_1..N` (disambung) atau `{prefix}`; validasi; isi cache proses.
// Sekali per proses: pemanggilan berikutnya tidak membaca ulang environment.
int32_t xhl_config_load(const char* prefix, char** err);

// Accessor bertipe. Kembalian: XHL_OK, XHL_MISSING (kunci tidak ada),
// XHL_TYPE (tipe salah; pesan menyebut kunci), atau kode kegagalan lain.
int32_t xhl_config_u32(const char* key, uint32_t* out, char** err);
int32_t xhl_config_i32(const char* key, int32_t* out, char** err);
int32_t xhl_config_bool(const char* key, int32_t* out, char** err);
int32_t xhl_config_f64(const char* key, double* out, char** err);
int32_t xhl_config_string(const char* key, char** out, char** err);
int32_t xhl_config_string_list(const char* key, char*** out, size_t* n, char** err);

// Rect: empat kunci angka. Ketika keempatnya absen -> XHL_MISSING.
// Bila sebagian ada dan sebagian tidak -> XHL_INVALID yang menyebut yang hilang
// (pola GetRect camoufox: rect separuh jadi lebih buruk daripada tidak ada).
int32_t xhl_config_rect(const char* kx, const char* ky, const char* kw, const char* kh,
                        uint32_t out[4], char** err);

// Seluruh config yang sudah tervalidasi, sebagai JSON. Untuk `xhl native config`.
int32_t xhl_config_dump(char** out_json, char** err);

// Nama kunci yang dikenal (tanpa nilainya). Dipakai MCP: nilai config tidak
// boleh keluar ke agent.
int32_t xhl_config_known_keys(char*** out, size_t* n, char** err);

// Bila config dimuat tetapi JSON-nya tidak sah, pesan kegagalan aslinya
// (agar `xhl native config` dapat melaporkan sebabnya, bukan config kosong).
int32_t xhl_config_load_error(char** out, char** err);

// Kunci yang **ada** tetapi tipenya salah. Mengembalikan 0 dengan daftar
// kosong bila semuanya cocok; daftar tidak kosong berarti konfigurasi akan
// diam-diam diabaikan saat dipakai — itu yang harus dilaporkan lebih dulu,
// bukan ditunggu sampai perilakunya salah.
int32_t xhl_config_validate(char*** bad_keys, size_t* n, char** err);

} // extern "C"