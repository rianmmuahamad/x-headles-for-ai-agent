// Skema profil penyamaran (adaptasi model data camoufox) + validasi.
//
// Representasi profil adalah JSON tervalidasi, bukan struct C++ terpisah:
// seluruh konsumen (koherensi di coherence.cpp, header di antidetect.cpp,
// MCP/CLI) bekerja pada JSON yang sama, sehingga round-trip `dump`->`validate`
// tidak melewati konversi tipe yang bisa kehilangan field. Aturan validasinya
// sendiri yang penting, dan itu yang diport dari accessor MaskConfig.

#pragma once

#include "common.hpp"

extern "C" {

// Validasi JSON profil tanpa menyimpannya. Pesan error menyebut field.
int32_t xhl_profile_validate(const char* json, char** err);

// Validasi lalu simpan sebagai profil proses. Menggantikan profil sebelumnya.
int32_t xhl_profile_from_json(const char* json, char** err);

// Hapus profil proses.
void xhl_profile_clear(void);

// Serialisasi profil yang tersimpan. XHL_STATE bila belum ada profil.
int32_t xhl_profile_dump(char** out_json, char** err);

// True (1) bila ada profil tersimpan.
int32_t xhl_profile_loaded(void);

} // extern "C"