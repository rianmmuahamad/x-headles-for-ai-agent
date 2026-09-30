// Pembaca JSON bertipe untuk kunci bergaya camoufox.
//
// Semantik "falsy" di sini mengikuti Python `if not x`: nilai 0/0.0/string
// kosong/absen dianggap kosong. Itu penting karena aturan koherensi yang
// diport dari camoufox memakai `if not width` dsb.

#pragma once

#include "json.hpp"

#include <optional>
#include <string>

namespace xhl {

inline bool has(const nlohmann::json& j, const char* key) {
  return j.is_object() && j.find(key) != j.end();
}

inline const nlohmann::json* node(const nlohmann::json& j, const char* key) {
  if (!j.is_object()) return nullptr;
  auto it = j.find(key);
  if (it == j.end() || it->is_discarded()) return nullptr;
  return &(*it);
}

// Integer JSON (menolak boolean dan pecahan), meniru `isinstance(x, int)`.
inline std::optional<int64_t> get_int(const nlohmann::json& j, const char* key) {
  const nlohmann::json* n = node(j, key);
  if (!n) return std::nullopt;
  if (n->is_number_unsigned()) return static_cast<int64_t>(n->get<uint64_t>());
  if (n->is_number_integer()) return n->get<int64_t>();
  return std::nullopt;
}

inline std::optional<double> get_double(const nlohmann::json& j, const char* key) {
  const nlohmann::json* n = node(j, key);
  if (!n || !n->is_number()) return std::nullopt;
  return n->get<double>();
}

inline std::optional<std::string> get_str(const nlohmann::json& j, const char* key) {
  const nlohmann::json* n = node(j, key);
  if (!n || !n->is_string()) return std::nullopt;
  return n->get_ref<const std::string&>();
}

inline std::optional<bool> get_bool(const nlohmann::json& j, const char* key) {
  const nlohmann::json* n = node(j, key);
  if (!n || !n->is_boolean()) return std::nullopt;
  return n->get<bool>();
}

// Ganti nilai kunci. Membuat objek bila `j` bukan objek.
inline void set(nlohmann::json& j, const char* key, const nlohmann::json& value) {
  if (!j.is_object()) j = nlohmann::json::object();
  j[key] = value;
}

inline void erase(nlohmann::json& j, const char* key) {
  if (j.is_object()) j.erase(key);
}

// True bila nilai absen ATAU bernilai nol/kosong (semantik `if not x`).
inline bool falsy_int(const nlohmann::json& j, const char* key) {
  auto v = get_int(j, key);
  return !v || *v == 0;
}

inline bool falsy_double(const nlohmann::json& j, const char* key) {
  auto v = get_double(j, key);
  return !v || *v == 0.0;
}

} // namespace xhl