// Persentil, lebar teks, dan penulisan frame BMP 24-bit.

#include "util.hpp"

#include <algorithm>
#include <cstdio>
#include <vector>

namespace {

// ---------------------------------------------------------------------------
// Font bitmap 5x7.
//
// Tabel ini ditulis untuk xhl: xhl adalah binary headless tanpa dependensi
// grafis, dan tujuannya hanya menggambar teks pendek yang dapat diperiksa
// secara visual (mis. label frame). 5x7 cukup untuk ASCII yang dapat dibaca.
//
// Setiap glyph adalah 5 kolom; tiap kolom adalah bitmask 7 baris, bit 0 =
// baris teratas. Mendefinisikan lewat string pola membuat tabel ini dapat
// diperiksa mata, bukan deretan angka heksadesimal tanpa makna.
// ---------------------------------------------------------------------------

struct Glyph {
  char c;
  const char* rows[7]; // 7 string, masing-masing 5 karakter ('#' atau '.')
};

// Hanya karakter yang dibutuhkan; sisanya memakai glyph kotak.
const Glyph kGlyphs[] = {
    {' ', {"     ", "     ", "     ", "     ", "     ", "     ", "     "}},
    {'-', {"     ", "     ", "     ", "#####", "     ", "     ", "     "}},
    {'.', {"     ", "     ", "     ", "     ", "     ", "     ", "  #  "}},
    {':', {"     ", "  #  ", "     ", "     ", "     ", "  #  ", "     "}},
    {'0', {" ### ", "#   #", "#  ##", "# # #", "##  #", "#   #", " ### "}},
    {'1', {"  #  ", " ##  ", "  #  ", "  #  ", "  #  ", "  #  ", " ### "}},
    {'2', {" ### ", "#   #", "    #", "   # ", "  #  ", " #   ", "#####"}},
    {'3', {"#####", "   # ", "  #  ", "   # ", "    #", "#   #", " ### "}},
    {'4', {"   # ", "  ## ", " # # ", "#  # ", "#####", "   # ", "   # "}},
    {'5', {"#####", "#    ", "#### ", "    #", "    #", "#   #", " ### "}},
    {'6', {"  ## ", " #   ", "#    ", "#### ", "#   #", "#   #", " ### "}},
    {'7', {"#####", "    #", "   # ", "  #  ", " #   ", " #   ", " #   "}},
    {'8', {" ### ", "#   #", "#   #", " ### ", "#   #", "#   #", " ### "}},
    {'9', {" ### ", "#   #", "#   #", " ####", "    #", "   # ", " ##  "}},
    {'A', {" ### ", "#   #", "#   #", "#####", "#   #", "#   #", "#   #"}},
    {'B', {"#### ", "#   #", "#   #", "#### ", "#   #", "#   #", "#### "}},
    {'C', {" ### ", "#   #", "#    ", "#    ", "#    ", "#   #", " ### "}},
    {'D', {"###  ", "#  # ", "#   #", "#   #", "#   #", "#  # ", "###  "}},
    {'E', {"#####", "#    ", "#    ", "#### ", "#    ", "#    ", "#####"}},
    {'F', {"#####", "#    ", "#    ", "#### ", "#    ", "#    ", "#    "}},
    {'G', {" ### ", "#   #", "#    ", "# ###", "#   #", "#   #", " ####"}},
    {'H', {"#   #", "#   #", "#   #", "#####", "#   #", "#   #", "#   #"}},
    {'I', {" ### ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", " ### "}},
    {'J', {"    #", "    #", "    #", "    #", "#   #", "#   #", " ### "}},
    {'K', {"#   #", "#  # ", "# #  ", "##   ", "# #  ", "#  # ", "#   #"}},
    {'L', {"#    ", "#    ", "#    ", "#    ", "#    ", "#    ", "#####"}},
    {'M', {"#   #", "## ##", "# # #", "#   #", "#   #", "#   #", "#   #"}},
    {'N', {"#   #", "##  #", "# # #", "#  ##", "#   #", "#   #", "#   #"}},
    {'O', {" ### ", "#   #", "#   #", "#   #", "#   #", "#   #", " ### "}},
    {'P', {"#### ", "#   #", "#   #", "#### ", "#    ", "#    ", "#    "}},
    {'Q', {" ### ", "#   #", "#   #", "#   #", "# # #", "#  # ", " ## #"}},
    {'R', {"#### ", "#   #", "#   #", "#### ", "# #  ", "#  # ", "#   #"}},
    {'S', {" ####", "#    ", "#    ", " ### ", "    #", "    #", "#### "}},
    {'T', {"#####", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  "}},
    {'U', {"#   #", "#   #", "#   #", "#   #", "#   #", "#   #", " ### "}},
    {'V', {"#   #", "#   #", "#   #", "#   #", "#   #", " # # ", "  #  "}},
    {'W', {"#   #", "#   #", "#   #", "# # #", "# # #", "## ##", "#   #"}},
    {'X', {"#   #", "#   #", " # # ", "  #  ", " # # ", "#   #", "#   #"}},
    {'Y', {"#   #", "#   #", " # # ", "  #  ", "  #  ", "  #  ", "  #  "}},
    {'Z', {"#####", "    #", "   # ", "  #  ", " #   ", "#    ", "#####"}},
};

constexpr int kGlyphW = 5;
constexpr int kGlyphH = 7;
constexpr int kSpacing = 1;

const Glyph* lookup(unsigned char c) {
  // ASCII huruf kecil dipetakan ke kapital agar tabel tetap kecil.
  if (c >= 'a' && c <= 'z') c = static_cast<unsigned char>(c - 'a' + 'A');
  for (const Glyph& g : kGlyphs) {
    if (static_cast<unsigned char>(g.c) == c) return &g;
  }
  return nullptr; // Pemanggil menggambar kotak kosong.
}

// Iterator rune UTF-8 minimal: mengembalikan code point + panjang byte.
struct RuneIter {
  const unsigned char* p;
  const unsigned char* end;

  // Mengembalikan false bila habis.
  bool next(uint32_t* cp, size_t* len) {
    if (p >= end) return false;
    const unsigned char b = *p;
    size_t n = 1;
    uint32_t v = b;
    if (b >= 0xF0) {
      n = 4;
      v = b & 0x07;
    } else if (b >= 0xE0) {
      n = 3;
      v = b & 0x0F;
    } else if (b >= 0xC0) {
      n = 2;
      v = b & 0x1F;
    }
    if (p + n > end) n = 1; // Urutan terpotong: perlakukan sebagai 1 byte.
    for (size_t i = 1; i < n; i++) {
      v = (v << 6) | (p[i] & 0x3F);
    }
    *cp = v;
    *len = n;
    p += n;
    return true;
  }
};

// Tulis header + piksel BMP 24-bit. Baris disimpan bottom-up dan setiap baris
// dipad ke kelipatan 4 byte, sesuai spesifikasi BMP.
bool write_bmp(const char* path, const std::vector<uint8_t>& pixels, int w, int h,
               std::string* err) {
  if (w <= 0 || h <= 0) {
    if (err) *err = "ukuran kanvas nol";
    return false;
  }
  const int row_bytes = w * 3;
  const int padding = (4 - (row_bytes % 4)) % 4;
  const uint32_t image_size = static_cast<uint32_t>((row_bytes + padding) * h);
  const uint32_t file_size = 54 + image_size;

  FILE* f = std::fopen(path, "wb");
  if (!f) {
    if (err) *err = std::string("tidak dapat membuka berkas keluaran: ") + path;
    return false;
  }

  uint8_t header[54] = {0};
  header[0] = 'B';
  header[1] = 'M';
  auto put32 = [&header](int off, uint32_t v) {
    header[off] = static_cast<uint8_t>(v & 0xFF);
    header[off + 1] = static_cast<uint8_t>((v >> 8) & 0xFF);
    header[off + 2] = static_cast<uint8_t>((v >> 16) & 0xFF);
    header[off + 3] = static_cast<uint8_t>((v >> 24) & 0xFF);
  };
  auto put16 = [&header](int off, uint16_t v) {
    header[off] = static_cast<uint8_t>(v & 0xFF);
    header[off + 1] = static_cast<uint8_t>((v >> 8) & 0xFF);
  };
  put32(2, file_size);
  put32(10, 54);        // offset data piksel
  put32(14, 40);        // ukuran header DIB
  put32(18, static_cast<uint32_t>(w));
  put32(22, static_cast<uint32_t>(h));
  put16(26, 1);         // planes
  put16(28, 24);        // bit per piksel
  put32(34, image_size);

  bool ok = std::fwrite(header, 1, 54, f) == 54;
  // BMP menyimpan baris dari bawah ke atas.
  const uint8_t pad[3] = {0, 0, 0};
  for (int y = h - 1; y >= 0 && ok; y--) {
    const uint8_t* row = pixels.data() + static_cast<size_t>(y) * row_bytes;
    ok = std::fwrite(row, 1, row_bytes, f) == static_cast<size_t>(row_bytes);
    if (ok && padding) ok = std::fwrite(pad, 1, padding, f) == static_cast<size_t>(padding);
  }
  std::fclose(f);
  if (!ok && err) *err = std::string("gagal menulis berkas keluaran: ") + path;
  return ok;
}

} // namespace

extern "C" {

int32_t xhl_percentile(const uint64_t* samples, size_t n, double p, double* out, char** err) {
  if (!out) {
    xhl::set_err(err, "xhl_percentile: argumen null");
    return XHL_INVALID;
  }
  if (n == 0) {
    xhl::set_err(err, "xhl_percentile: tidak ada sampel");
    return XHL_INVALID;
  }
  if (p < 0.0 || p > 1.0) {
    xhl::set_err(err, "xhl_percentile: p harus di [0,1]");
    return XHL_INVALID;
  }
  std::vector<uint64_t> v(samples, samples + n);
  std::sort(v.begin(), v.end());
  if (n == 1) {
    *out = static_cast<double>(v[0]);
    return XHL_OK;
  }
  // Interpolasi linear: posisi = p * (n-1).
  const double pos = p * static_cast<double>(n - 1);
  const size_t lo = static_cast<size_t>(pos);
  const size_t hi = (lo + 1 < n) ? lo + 1 : lo;
  const double frac = pos - static_cast<double>(lo);
  *out = static_cast<double>(v[lo]) * (1.0 - frac) + static_cast<double>(v[hi]) * frac;
  return XHL_OK;
}

int32_t xhl_text_width(const char* utf8, uint32_t* out_cols, char** err) {
  if (!utf8 || !out_cols) {
    xhl::set_err(err, "xhl_text_width: argumen null");
    return XHL_INVALID;
  }
  RuneIter it{reinterpret_cast<const unsigned char*>(utf8),
              reinterpret_cast<const unsigned char*>(utf8) + std::strlen(utf8)};
  uint32_t cols = 0;
  uint32_t cp = 0;
  size_t len = 0;
  while (it.next(&cp, &len)) {
    cols += (cp < 128 && lookup(static_cast<unsigned char>(cp))) ? (kGlyphW + kSpacing) : 1;
  }
  *out_cols = cols;
  return XHL_OK;
}

int32_t xhl_draw_frame(const char* utf8, const char* path, uint32_t scale, char** err) {
  if (!utf8 || !path) {
    xhl::set_err(err, "xhl_draw_frame: argumen null");
    return XHL_INVALID;
  }
  if (scale == 0) scale = 1;

  RuneIter it{reinterpret_cast<const unsigned char*>(utf8),
              reinterpret_cast<const unsigned char*>(utf8) + std::strlen(utf8)};
  const int cw = (kGlyphW + kSpacing) * static_cast<int>(scale);
  const int ch = (kGlyphH + 2) * static_cast<int>(scale);
  const int width = cw * 8;
  const int height = ch * 3;
  std::vector<uint8_t> pixels(static_cast<size_t>(width) * height * 3, 0xFF); // putih

  int cursor_row = 0;
  int x = 2;
  int y = 2;
  uint32_t cp = 0;
  size_t len = 0;
  while (it.next(&cp, &len)) {
    if (cp == '\n') {
      cursor_row++;
      y = 2 + cursor_row * ch;
      x = 2;
      continue;
    }
    const Glyph* g = (cp < 128) ? lookup(static_cast<unsigned char>(cp)) : nullptr;
    for (int gy = 0; gy < kGlyphH; gy++) {
      for (int gx = 0; gx < kGlyphW; gx++) {
        const bool on = g && g->rows[gy][gx] == '#';
        if (!on) continue;
        for (uint32_t sy = 0; sy < scale; sy++) {
          for (uint32_t sx = 0; sx < scale; sx++) {
            const int px = x + gx * static_cast<int>(scale) + static_cast<int>(sx);
            const int py = y + gy * static_cast<int>(scale) + static_cast<int>(sy);
            if (px < 0 || py < 0 || px >= width || py >= height) continue;
            uint8_t* p = pixels.data() + (static_cast<size_t>(py) * width + px) * 3;
            p[0] = 0x00; // B
            p[1] = 0x00; // G
            p[2] = 0x00; // R
          }
        }
      }
    }
    x += cw;
    if (x + cw > width) {
      cursor_row++;
      y = 2 + cursor_row * ch;
      x = 2;
    }
  }

  std::string werr;
  if (!write_bmp(path, pixels, width, height, &werr)) {
    xhl::set_err(err, werr);
    return XHL_IO;
  }
  return XHL_OK;
}

} // extern "C"