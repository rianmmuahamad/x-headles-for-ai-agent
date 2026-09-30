// Header HTTP hasil derivasi profil anti-detect.
//
// Ini bagian profil yang **benar-benar dipakai transport**: nama header dan
// nilainya. Camoufox melakukan hal yang sama di lapisan jaringannya
// (`patches/network-patches.patch` menambal nsHttpHandler dan membaca
// `headers.User-Agent`, `headers.Accept-Language`, `headers.Accept-Encoding`).
//
// `headers.User-Agent` lebih kuat daripada `navigator.userAgent`; itu perilaku
// sumbernya (User-Agent pertama yang diperiksa, navigator hanya cadangan).

#pragma once

#include "common.hpp"

extern "C" {

typedef struct {
  const char* user_agent;      // headers.User-Agent, cadangan navigator.userAgent
  const char* accept_language; // headers.Accept-Language
  const char* accept_encoding; // headers.Accept-Encoding
  const char* accept;          // headers.Accept
  const char* dnt;             // headers.DNT
  const char* viewport_width;  // headers.Viewport-Width
  const char* sec_ch_ua;       // headers.Sec-CH-UA
  const char* sec_ch_ua_mobile; // headers.Sec-CH-UA-Mobile
  const char* sec_ch_ua_platform; // headers.Sec-CH-UA-Platform
  const char** order;          // urutan header (nullptr -> pakai bawaan)
  size_t order_n;
  // Asal nilai: 0 = tidak ada (pakai bawaan xhl), 1 = dari profil.
  int32_t from_profile;
  int32_t from_navigator_fallback; // 1 bila user_agent dari navigator.userAgent
} xhl_headers;

// Derivasi header dari JSON profil. Kunci yang tidak ada tetap nullptr.
int32_t xhl_headers_from_profile(const char* profile_json, xhl_headers* out, char** err);

void xhl_headers_free(xhl_headers* h);

} // extern "C"