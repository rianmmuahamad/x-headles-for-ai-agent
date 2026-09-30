// Utilitas lepas-browser: persentil, lebar teks, dan frame gambar.
//
// Ketiganya tidak menyentuh browser maupun jaringan, sehingga aman dijalankan
// di mesin tanpa layar.

#pragma once

#include "common.hpp"

extern "C" {

// Persentil dari sampel (p di [0,1]) dengan interpolasi linear antar tetangga.
// Sampel tidak perlu terurut: fungsi menyalin lalu mengurutkan.
// p50 dari 1..100 harus menghasilkan 50.5.
int32_t xhl_percentile(const uint64_t* samples, size_t n, double p, double* out, char** err);

// Lebar teks menurut metrik font bitmap 5x7 (lihat util.cpp):
// rune ASCII yang punya glyph dihitung 1 kolom + 1 kolom jarak; rune lain
// dihitung 1 kolom.
int32_t xhl_text_width(const char* utf8, uint32_t* out_cols, char** err);

// Gambar teks ke kanvas lalu tulis sebagai BMP 24-bit (tanpa kompresi, tanpa
// zlib). `scale` memperbesar tiap piksel glyph (1 = ukuran asli).
int32_t xhl_draw_frame(const char* utf8, const char* path, uint32_t scale, char** err);

} // extern "C"