// Titik masuk inti C++: versi + pembebasan memori lintas batas FFI.
//
// Pembebasan ada di sini (bukan di tiap modul) karena Rust hanya boleh
// memanggil satu pasangan fungsi bebas untuk semua alokasi C++.

#include "common.hpp"

extern "C" {

// Versi inti native, untuk `xhl native info` dan MCP.
const char* xhl_native_version(void) {
  // Disimpan statis: pemanggil tidak membebaskan hasil ini.
  static const char* kVersion = "xhl-native/1 (c++17)";
  return kVersion;
}

void xhl_string_free(char* s) { std::free(s); }

void xhl_strings_free(char** arr, size_t n) { xhl::free_strings(arr, n); }

} // extern "C"