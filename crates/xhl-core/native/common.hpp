// Helper bersama untuk inti C++ xhl.
//
// Aturan alokasi lintas batas FFI: seluruh memori yang menyeberang ke Rust
// dialokasikan dengan malloc dan dibebaskan Rust lewat xhl_string_free /
// xhl_strings_free (keduanya memanggil free). Tidak ada std::string yang
// menyeberang batas.
#pragma once

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <string>
#include <string_view>

// Kode status FFI. Dipetakan di crates/xhl-core/src/native/ffi.rs.
// Nilai negatif khusus accessor config: membedakan "kunci tidak ada" dari
// "kunci ada tapi tipenya salah", seperti CheckBool/GetString di MaskConfig.
//   -1 = kunci tidak ada
//   -2 = tipe salah
// Nilai positif adalah kegagalan nyata.
constexpr int32_t XHL_OK = 0;
constexpr int32_t XHL_MISSING = -1;
constexpr int32_t XHL_TYPE = -2;
constexpr int32_t XHL_STATE = 1;   // urutan pemanggilan salah (belum di-load)
constexpr int32_t XHL_PARSE = 2;   // JSON tidak sah
constexpr int32_t XHL_INVALID = 3; // nilai melanggar aturan validasi
constexpr int32_t XHL_ALLOC = 4;   // alokasi gagal
constexpr int32_t XHL_IO = 5;      // gagal baca/tulis berkas

namespace xhl {

// Salin string ke buffer malloc. Mengembalikan nullptr bila alokasi gagal
// (pemanggil memperlakukannya sebagai XHL_ALLOC).
inline char* dup(std::string_view s) {
  char* p = static_cast<char*>(std::malloc(s.size() + 1));
  if (!p) return nullptr;
  if (!s.empty()) std::memcpy(p, s.data(), s.size());
  p[s.size()] = '\0';
  return p;
}

// Isi *err dengan salinan pesan. Null-safe: pemanggil boleh mengirim nullptr
// bila tidak peduli pesannya.
inline void set_err(char** err, std::string_view msg) {
  if (err) *err = dup(msg);
}

// Bebaskan char** hasil alokasi (array + tiap elemen).
inline void free_strings(char** arr, size_t n) {
  if (!arr) return;
  for (size_t i = 0; i < n; i++) std::free(arr[i]);
  std::free(arr);
}

} // namespace xhl

// Fungsi pembebasan lintas batas FFI, didefinisikan di entry.cpp. Pesan error
// inline di atas sering perlu membebaskan larik yang dibangun di sana, jadi
// deklarasinya di sini, bukan hanya di entry.cpp.
extern "C" {
const char* xhl_native_version(void);
void xhl_string_free(char* s);
void xhl_strings_free(char** arr, size_t n);
}