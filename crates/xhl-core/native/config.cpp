// Config dari environment. Lihat config.hpp untuk alasan tiap keputusan.

#include "config.hpp"
#include "json.hpp"

#include <mutex>
#include <vector>

namespace {

// Prefix default. Dipakai untuk pesan error bila pemanggil mengirim nullptr.
constexpr const char* kDefaultPrefix = "XHL_CONFIG";

struct ConfigState {
  nlohmann::json json = nlohmann::json::object();
  bool loaded = false;
  std::string prefix = kDefaultPrefix;
  // Pesan kegagalan penguraian, bila ada. Dibiarkan sehingga pemanggil dapat
  // melaporkan *sebabnya*, bukan hanya config kosong.
  std::string load_error;
};

ConfigState& state() {
  // Statis fungsi: satu instance per proses, tanpa urutan inisialisasi global.
  static ConfigState s;
  return s;
}

std::once_flag& once() {
  static std::once_flag f;
  return f;
}

// Kunci yang dikenal xhl beserta tipenya. Daftar ini dipakai
// `xhl_config_known_keys` dan `xhl_config_validate`, sehingga kunci baru
// didaftarkan satu kali dan langsung ikut divalidasi.
enum KeyType { TK_STRING, TK_BOOL, TK_U32 };

struct KnownKey {
  const char* name;
  KeyType type;
};

const KnownKey kKnownKeys[] = {
    {"http_profile", TK_STRING},
    {"no_wait", TK_BOOL},
    {"llm_model", TK_STRING},
    {"query_ids_path", TK_STRING},
    {"content_max_bytes", TK_U32},
    {"retry_attempts", TK_U32},
    {"retry_backoff_ms", TK_U32},
    {"request_timeout_secs", TK_U32},
    {"discovery_concurrency", TK_U32},
    {"media_chunk_bytes", TK_U32},
    {"emulation_profile", TK_STRING},
    {"profile", TK_STRING},
    {"account", TK_STRING},
    {"draw_x", TK_U32},
    {"draw_y", TK_U32},
    {"draw_w", TK_U32},
    {"draw_h", TK_U32},
};

// Var env tunggal yang didukung xhl sebelum ada `XHL_CONFIG`. Dipetakan ke
// kunci JSON yang sama, dan hanya dipakai bila kunci itu belum ada di JSON —
// sehingga `XHL_CONFIG` selalu menang.
//
// Pembacaan env sengaja tinggal di lapisan ini, bukan di Rust: satu tempat
// konfigurasi dibaca berarti satu tempat yang perlu diuji, dan perilaku lama
// tetap bekerja tanpa jalur cadangan yang terpisah di sisi Rust.
struct LegacyEnv {
  const char* env;
  const char* key;
};

const LegacyEnv kLegacyEnv[] = {
    {"XHL_ACCOUNT", "account"},
    {"XHL_HTTP_PROFILE", "http_profile"},
    {"XHL_QUERY_IDS", "query_ids_path"},
    {"XHL_LLM_MODEL", "llm_model"},
};

// Baca env var sebagai UTF-8. nullptr bila tidak ada.
const char* get_env(const std::string& name) { return std::getenv(name.c_str()); }

// Nilai env "benar": sama dengan yang diterima xhl sebelum cutover
// (`v == "1" || v.eq_ignore_ascii_case("true")`).
bool env_is_true(const char* v) {
  if (!v) return false;
  if (v[0] == '1' && v[1] == '\0') return true;
  const char* t = "true";
  for (int i = 0; i < 4; i++) {
    char c = v[i];
    if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
    if (c != t[i]) return false;
  }
  return v[4] == '\0';
}

void apply_legacy_env() {
  ConfigState& s = state();
  if (!s.json.is_object()) s.json = nlohmann::json::object();
  for (const LegacyEnv& le : kLegacyEnv) {
    if (s.json.find(le.key) != s.json.end()) continue; // JSON menang
    const char* v = get_env(le.env);
    if (v && *v) s.json[le.key] = std::string(v);
  }
  // `XHL_NO_WAIT` hanya bernilai bila env-nya ada; absennya berarti false,
  // sama seperti `unwrap_or(false)` sebelumnya.
  if (s.json.find("no_wait") == s.json.end()) {
    if (const char* v = get_env("XHL_NO_WAIT")) s.json["no_wait"] = env_is_true(v);
  }
}

void do_load(const std::string& prefix) {
  ConfigState& s = state();
  s.prefix = prefix;

  // `_1..N` disambung sebagai teks, mengikuti MaskConfig: pemanggil dapat
  // memecah satu dokumen JSON panjang melewati batas panjang env var.
  //
  // Bila hasil sambungan bukan JSON sah, barulah objeknya digabung (kunci dari
  // var bernomor lebih besar menang). Tanpa jalur kedua ini, pemanggil yang
  // wajar menulis `XHL_CONFIG_1='{"a":1}' XHL_CONFIG_2='{"b":2}'` akan mendapat
  // "Invalid JSON" padahal maksudnya jelas — dan pesan itu tidak menjelaskan
  // apa pun tentang semantik sambungan.
  std::vector<std::string> parts;
  {
    int i = 1;
    while (true) {
      const char* part = get_env(prefix + "_" + std::to_string(i));
      if (!part) break;
      parts.push_back(part);
      i++;
    }
  }
  std::string combined;
  for (const std::string& p : parts) combined += p;

  const bool concatenated_ok = !combined.empty() && nlohmann::json::accept(combined);
  if (!concatenated_ok && parts.size() > 1) {
    nlohmann::json merged = nlohmann::json::object();
    bool all_objects = true;
    for (const std::string& p : parts) {
      if (!nlohmann::json::accept(p)) {
        all_objects = false;
        break;
      }
      const nlohmann::json one = nlohmann::json::parse(p, nullptr, false);
      if (one.is_discarded() || !one.is_object()) {
        all_objects = false;
        break;
      }
      for (auto it = one.begin(); it != one.end(); ++it) merged[it.key()] = it.value();
    }
    if (all_objects) {
      s.json = std::move(merged);
      apply_legacy_env();
      return;
    }
  }

  if (combined.empty()) {
    if (const char* whole = get_env(prefix)) combined = whole;
  }
  if (combined.empty()) {
    // Tanpa `XHL_CONFIG*`: masih ada var env tunggal lama yang harus dihormati.
    apply_legacy_env();
    return;
  }

  if (!nlohmann::json::accept(combined)) {
    // Ditiru dari MaskConfig: JSON tidak sah -> config kosong + pesan, bukan
    // terminasi. Pesannya menyebut semantik sambungan agar dapat ditindak.
    s.load_error = std::string("Invalid JSON passed to ") + prefix +
                   " (XHL_CONFIG_1..N disambung sebagai teks JSON; bila memakai "
                   "beberapa var, isi tiap var harus objek JSON yang sah)";
    return;
  }

  // `allow_exceptions = false`: dengan -fno-exceptions, parse tidak boleh
  // melempar; `accept` di atas sudah menjamin masukan sah.
  s.json = nlohmann::json::parse(combined, nullptr, false);
  if (s.json.is_discarded()) {
    s.load_error = std::string("Invalid JSON passed to ") + prefix;
    s.json = nlohmann::json::object();
    return;
  }
  if (!s.json.is_object()) {
    s.load_error = std::string("Config for ") + prefix + " must be a JSON object";
    s.json = nlohmann::json::object();
  }
  // Var env tunggal lama tetap berlaku untuk kunci yang tidak disebut JSON.
  apply_legacy_env();
}

// Ambil node kunci. Mengembalikan nullptr bila tidak ada atau bukan objek.
const nlohmann::json* find(const char* key) {
  ConfigState& s = state();
  if (!s.json.is_object()) return nullptr;
  auto it = s.json.find(key);
  if (it == s.json.end()) return nullptr;
  return &(*it);
}

// Ambil node kunci, menolak nilai discarded (JSON_NOEXCEPTION).
const nlohmann::json* safe_get(const char* key) {
  const nlohmann::json* j = find(key);
  if (!j) return nullptr;
  if (j->is_discarded()) return nullptr;
  return j;
}

// Pemetaan kode tipe nlohmann -> nama untuk pesan error yang dapat ditindak.
const char* type_name(const nlohmann::json& j) {
  if (j.is_null()) return "null";
  if (j.is_boolean()) return "boolean";
  if (j.is_number_integer()) return "integer";
  if (j.is_number_unsigned()) return "unsigned integer";
  if (j.is_number_float()) return "number";
  if (j.is_string()) return "string";
  if (j.is_array()) return "array";
  if (j.is_object()) return "object";
  return "unknown";
}

// Pesan gagal tipe, mengikuti gaya MaskConfig ("Value for key '%s' is not ...").
void type_error(char** err, const char* key, const char* expected,
                const nlohmann::json& got) {
  if (!err) return;
  std::string msg = "Value for key '";
  msg += key;
  msg += "' is not ";
  msg += expected;
  msg += " (got ";
  msg += type_name(got);
  msg += ")";
  xhl::set_err(err, msg);
}

} // namespace

extern "C" {

int32_t xhl_config_load(const char* prefix, char** err) {
  // Tanpa exception (-fno-exceptions): `accept` + `allow_exceptions=false`
  // membuat penguraian tidak dapat melempar.
  const std::string p = (prefix && *prefix) ? prefix : kDefaultPrefix;
  std::call_once(once(), [&p]() { do_load(p); });
  ConfigState& s = state();
  if (s.loaded) return XHL_OK;
  s.loaded = true;
  if (!s.load_error.empty()) {
    xhl::set_err(err, s.load_error);
    return XHL_PARSE;
  }
  return XHL_OK;
}

int32_t xhl_config_u32(const char* key, uint32_t* out, char** err) {
  if (!key || !out) {
    xhl::set_err(err, "xhl_config_u32: argumen null");
    return XHL_INVALID;
  }
  const nlohmann::json* j = safe_get(key);
  if (!j) return XHL_MISSING;
  // Nilai di luar jangkauan uint32 ditolak, bukan dipotong diam-diam.
  int64_t raw = 0;
  if (j->is_number_unsigned()) {
    const uint64_t u = j->get<uint64_t>();
    if (u > UINT32_MAX) {
      xhl::set_err(err, "Value for key '" + std::string(key) + "' exceeds uint32 range");
      return XHL_TYPE;
    }
    raw = static_cast<int64_t>(u);
  } else if (j->is_number_integer()) {
    raw = j->get<int64_t>();
    if (raw < 0 || raw > static_cast<int64_t>(UINT32_MAX)) {
      xhl::set_err(err, "Value for key '" + std::string(key) + "' is not an unsigned integer (got " +
                            std::to_string(raw) + ")");
      return XHL_TYPE;
    }
  } else {
    type_error(err, key, "an unsigned integer", *j);
    return XHL_TYPE;
  }
  *out = static_cast<uint32_t>(raw);
  return XHL_OK;
}

int32_t xhl_config_i32(const char* key, int32_t* out, char** err) {
  if (!key || !out) {
    xhl::set_err(err, "xhl_config_i32: argumen null");
    return XHL_INVALID;
  }
  const nlohmann::json* j = safe_get(key);
  if (!j) return XHL_MISSING;
  if (!j->is_number_integer()) {
    type_error(err, key, "an integer", *j);
    return XHL_TYPE;
  }
  const int64_t raw = j->get<int64_t>();
  if (raw < INT32_MIN || raw > INT32_MAX) {
    xhl::set_err(err, "Value for key '" + std::string(key) + "' exceeds int32 range");
    return XHL_TYPE;
  }
  *out = static_cast<int32_t>(raw);
  return XHL_OK;
}

int32_t xhl_config_bool(const char* key, int32_t* out, char** err) {
  if (!key || !out) {
    xhl::set_err(err, "xhl_config_bool: argumen null");
    return XHL_INVALID;
  }
  const nlohmann::json* j = safe_get(key);
  if (!j) return XHL_MISSING;
  if (!j->is_boolean()) {
    type_error(err, key, "a boolean", *j);
    return XHL_TYPE;
  }
  *out = j->get<bool>() ? 1 : 0;
  return XHL_OK;
}

int32_t xhl_config_f64(const char* key, double* out, char** err) {
  if (!key || !out) {
    xhl::set_err(err, "xhl_config_f64: argumen null");
    return XHL_INVALID;
  }
  const nlohmann::json* j = safe_get(key);
  if (!j) return XHL_MISSING;
  if (!j->is_number()) {
    type_error(err, key, "a number", *j);
    return XHL_TYPE;
  }
  *out = j->get<double>();
  return XHL_OK;
}

int32_t xhl_config_string(const char* key, char** out, char** err) {
  if (!key || !out) {
    xhl::set_err(err, "xhl_config_string: argumen null");
    return XHL_INVALID;
  }
  *out = nullptr;
  const nlohmann::json* j = safe_get(key);
  if (!j) return XHL_MISSING;
  if (!j->is_string()) {
    type_error(err, key, "a string", *j);
    return XHL_TYPE;
  }
  const std::string& v = j->get_ref<const std::string&>();
  char* p = xhl::dup(v);
  if (!p) {
    xhl::set_err(err, "alokasi gagal untuk nilai string");
    return XHL_ALLOC;
  }
  *out = p;
  return XHL_OK;
}

int32_t xhl_config_string_list(const char* key, char*** out, size_t* n, char** err) {
  if (!key || !out || !n) {
    xhl::set_err(err, "xhl_config_string_list: argumen null");
    return XHL_INVALID;
  }
  *out = nullptr;
  *n = 0;
  const nlohmann::json* j = safe_get(key);
  if (!j) return XHL_MISSING;
  if (!j->is_array()) {
    type_error(err, key, "an array of strings", *j);
    return XHL_TYPE;
  }
  const size_t len = j->size();
  if (len == 0) return XHL_OK;
  char** arr = static_cast<char**>(std::calloc(len, sizeof(char*)));
  if (!arr) {
    xhl::set_err(err, "alokasi gagal untuk daftar string");
    return XHL_ALLOC;
  }
  size_t filled = 0;
  for (const nlohmann::json& item : *j) {
    if (!item.is_string()) {
      xhl::free_strings(arr, len);
      type_error(err, key, "an array of strings", item);
      return XHL_TYPE;
    }
    arr[filled] = xhl::dup(item.get_ref<const std::string&>());
    if (!arr[filled]) {
      xhl::free_strings(arr, len);
      xhl::set_err(err, "alokasi gagal untuk elemen daftar");
      return XHL_ALLOC;
    }
    filled++;
  }
  *out = arr;
  *n = filled;
  return XHL_OK;
}

int32_t xhl_config_rect(const char* kx, const char* ky, const char* kw, const char* kh,
                        uint32_t out[4], char** err) {
  if (!kx || !ky || !kw || !kh || !out) {
    xhl::set_err(err, "xhl_config_rect: argumen null");
    return XHL_INVALID;
  }
  const char* keys[4] = {kx, ky, kw, kh};
  uint32_t vals[4] = {0, 0, 0, 0};
  int present = 0;
  for (int i = 0; i < 4; i++) {
    uint32_t v = 0;
    const int32_t rc = xhl_config_u32(keys[i], &v, err);
    if (rc == XHL_OK) {
      vals[i] = v;
      present++;
    } else if (rc != XHL_MISSING) {
      return rc; // Tipe salah: pesan sudah menyebut kunci.
    }
  }
  if (present == 0) return XHL_MISSING;
  if (present != 4) {
    // Setengah rect lebih buruk daripada tiadanya rect: pemanggil dapat
    // menggambar dengan asal-asalan. Sebut kunci mana yang hilang.
    std::string msg = "rect tidak lengkap; kunci hilang:";
    for (int i = 0; i < 4; i++) {
      if (!safe_get(keys[i])) {
        msg += " ";
        msg += keys[i];
      }
    }
    xhl::set_err(err, msg);
    return XHL_INVALID;
  }
  for (int i = 0; i < 4; i++) out[i] = vals[i];
  return XHL_OK;
}

int32_t xhl_config_dump(char** out_json, char** err) {
  if (!out_json) {
    xhl::set_err(err, "xhl_config_dump: argumen null");
    return XHL_INVALID;
  }
  *out_json = nullptr;
  ConfigState& s = state();
  // `dump` tidak melempar. Indentasi 2 agar terbaca manusia di CLI.
  const std::string dumped = s.json.dump(2);
  char* p = xhl::dup(dumped);
  if (!p) {
    xhl::set_err(err, "alokasi gagal untuk dump config");
    return XHL_ALLOC;
  }
  *out_json = p;
  return XHL_OK;
}

int32_t xhl_config_known_keys(char*** out, size_t* n, char** err) {
  if (!out || !n) {
    xhl::set_err(err, "xhl_config_known_keys: argumen null");
    return XHL_INVALID;
  }
  *out = nullptr;
  *n = 0;
  const size_t len = sizeof(kKnownKeys) / sizeof(kKnownKeys[0]);
  char** arr = static_cast<char**>(std::calloc(len, sizeof(char*)));
  if (!arr) {
    xhl::set_err(err, "alokasi gagal untuk daftar kunci");
    return XHL_ALLOC;
  }
  for (size_t i = 0; i < len; i++) {
    arr[i] = xhl::dup(kKnownKeys[i].name);
    if (!arr[i]) {
      xhl::free_strings(arr, len);
      xhl::set_err(err, "alokasi gagal untuk elemen kunci");
      return XHL_ALLOC;
    }
  }
  *out = arr;
  *n = len;
  return XHL_OK;
}

int32_t xhl_config_load_error(char** out, char** err) {
  if (!out) {
    xhl::set_err(err, "xhl_config_load_error: argumen null");
    return XHL_INVALID;
  }
  *out = nullptr;
  ConfigState& s = state();
  if (s.load_error.empty()) return XHL_MISSING;
  char* p = xhl::dup(s.load_error);
  if (!p) {
    xhl::set_err(err, "alokasi gagal untuk pesan error");
    return XHL_ALLOC;
  }
  *out = p;
  return XHL_OK;
}

int32_t xhl_config_validate(char*** bad_keys, size_t* n, char** err) {
  if (!bad_keys || !n) {
    xhl::set_err(err, "xhl_config_validate: argumen null");
    return XHL_INVALID;
  }
  *bad_keys = nullptr;
  *n = 0;

  std::vector<std::string> bad;
  for (const KnownKey& k : kKnownKeys) {
    const nlohmann::json* j = safe_get(k.name);
    if (!j) continue;
    bool ok = true;
    switch (k.type) {
      case TK_STRING:
        ok = j->is_string();
        break;
      case TK_BOOL:
        ok = j->is_boolean();
        break;
      case TK_U32:
        // Sama dengan aturan xhl_config_u32: bilangan bulat tidak negatif dan
        // muat uint32.
        if (j->is_number_unsigned()) {
          ok = j->get<uint64_t>() <= UINT32_MAX;
        } else if (j->is_number_integer()) {
          const int64_t v = j->get<int64_t>();
          ok = v >= 0 && v <= static_cast<int64_t>(UINT32_MAX);
        } else {
          ok = false;
        }
        break;
    }
    if (!ok) bad.push_back(k.name);
  }

  if (bad.empty()) return XHL_OK;
  char** arr = static_cast<char**>(std::calloc(bad.size(), sizeof(char*)));
  if (!arr) {
    xhl::set_err(err, "alokasi gagal untuk daftar kunci salah tipe");
    return XHL_ALLOC;
  }
  for (size_t i = 0; i < bad.size(); i++) {
    arr[i] = xhl::dup(bad[i]);
    if (!arr[i]) {
      xhl::free_strings(arr, bad.size());
      xhl::set_err(err, "alokasi gagal untuk elemen kunci salah tipe");
      return XHL_ALLOC;
    }
  }
  *bad_keys = arr;
  *n = bad.size();
  return XHL_OK;
}

} // extern "C"