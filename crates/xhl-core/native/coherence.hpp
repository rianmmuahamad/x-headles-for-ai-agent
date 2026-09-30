// Koherensi identitas: aturan yang melihat lebih dari satu field sekaligus.
//
// Diport dari `pythonlib/camoufox/coherence.py`. Nama aturan dipertahankan
// persis agar dapat dibandingkan dengan sumbernya. Ambang batas dan himpunan
// nilai yang diperbolehkan disalin apa adanya, termasuk komentar rujukannya.
//
// Alasan fase ini ada: identitas disusun dari beberapa kolam nilai yang
// masing-masing wajar, tetapi gabungannya bisa mustahil (Apple M1 dengan 2
// core, GPU Intel Mac di balik panel Apple Silicon). Defect itu lahir saat
// penggabungan, sehingga tidak bisa dibersihkan di kolam masing-masing.

#pragma once

#include "common.hpp"

extern "C" {

typedef struct {
  const char* rule;   // nama aturan, sama seperti di coherence.py
  const char* detail; // alasan dalam bahasa manusia
} xhl_violation;

// Pelanggaran yang tersisa. `target_os`: "win" | "mac" | "lin".
int32_t xhl_coherence_validate(const char* profile_json, const char* target_os,
                               xhl_violation** out, size_t* n, char** err);

// Perbaiki yang dapat ditentukan, lalu laporkan yang **tersisa** (semantik
// `apply()` di sumber: `return validate(...)`). Keluaran JSON yang sudah
// diperbaiki sekaligus, agar pemanggil tidak perlu mem-parse ulang.
int32_t xhl_coherence_apply(const char* profile_json, const char* target_os, char** out_json,
                            xhl_violation** out, size_t* n, char** err);

// Buang nilai yang tidak dapat dipertahankan identitas (pola
// drop_incoherent_source_values): GPU yang tidak mungkin di OS itu.
int32_t xhl_coherence_drop_incoherent(const char* profile_json, const char* target_os,
                                      char** out_json, xhl_violation** out, size_t* n,
                                      char** err);

void xhl_violations_free(xhl_violation* v, size_t n);

} // extern "C"