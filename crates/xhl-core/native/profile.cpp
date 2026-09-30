// Validasi profil penyamaran. Lihat profile.hpp untuk alasannya.

#include "profile.hpp"
#include "jsonutil.hpp"

#include <mutex>
#include <string>
#include <vector>

namespace {

struct ProfileState {
  nlohmann::json json = nlohmann::json::object();
  bool loaded = false;
};

ProfileState& state() {
  static ProfileState s;
  return s;
}

// Field bilangan bulat tak-negatif. Negatif atau pecahan ditolak dengan pesan
// yang menyebut field, bukan dikonversi diam-diam.
const char* const kUintKeys[] = {
    "navigator.hardwareConcurrency", "navigator.maxTouchPoints",
    "screen.width",                  "screen.height",
    "screen.availWidth",             "screen.availHeight",
    "screen.colorDepth",             "screen.pixelDepth",
    "window.innerWidth",             "window.innerHeight",
    "window.outerWidth",             "window.outerHeight",
    "chrome.height",                 "audio.sampleRate",
};

// Field pecahan.
const char* const kNumberKeys[] = {
    "window.devicePixelRatio", "navigator.deviceMemory", "audio.fudgeFactor",
};

// Field teks.
const char* const kStringKeys[] = {
    "navigator.userAgent", "navigator.platform",    "navigator.oscpu",
    "webGl:vendor",        "webGl:renderer",        "webGl:version",
    "webGl:shadingLanguageVersion", "headers.User-Agent",
    "headers.Accept-Language",      "headers.Accept-Encoding",
    "locale.language",     "locale.region",         "timezone.timezone",
    "chrome.css",
};

int32_t fail(std::string msg, char** err) {
  xhl::set_err(err, msg);
  return XHL_INVALID;
}

// Validasi satu kunci sebagai uint bila ada.
int32_t check_uint(const nlohmann::json& p, const char* key, char** err) {
  const nlohmann::json* n = xhl::node(p, key);
  if (!n) return XHL_OK;
  if (n->is_number_unsigned()) return XHL_OK;
  if (n->is_number_integer()) {
    if (n->get<int64_t>() < 0) {
      return fail(std::string("field '") + key + "' tidak boleh negatif", err);
    }
    return XHL_OK;
  }
  return fail(std::string("field '") + key + "' harus bilangan bulat tak-negatif", err);
}

int32_t check_number(const nlohmann::json& p, const char* key, char** err) {
  const nlohmann::json* n = xhl::node(p, key);
  if (!n) return XHL_OK;
  if (!n->is_number()) {
    return fail(std::string("field '") + key + "' harus berupa angka", err);
  }
  return XHL_OK;
}

int32_t check_string(const nlohmann::json& p, const char* key, char** err) {
  const nlohmann::json* n = xhl::node(p, key);
  if (!n) return XHL_OK;
  if (!n->is_string()) {
    return fail(std::string("field '") + key + "' harus berupa teks", err);
  }
  return XHL_OK;
}

// Kunci shaderPrecisionFormats berformat "<int>,<int>" (koma boleh diikuti
// spasi) atau "<int>><int>", meniru MShaderData camoufox yang menerima keduanya.
bool parse_int_pair(const std::string& s, int64_t* a, int64_t* b) {
  size_t i = 0;
  auto skip_ws = [&s](size_t& i) {
    while (i < s.size() && (s[i] == ' ' || s[i] == '\t')) i++;
  };
  auto read_int = [&s](size_t& i, int64_t* out) -> bool {
    const size_t start = i;
    if (i < s.size() && (s[i] == '-' || s[i] == '+')) i++;
    while (i < s.size() && s[i] >= '0' && s[i] <= '9') i++;
    if (i == start) return false;
    *out = std::strtoll(s.substr(start, i - start).c_str(), nullptr, 10);
    return true;
  };
  skip_ws(i);
  if (!read_int(i, a)) return false;
  skip_ws(i);
  if (i >= s.size() || (s[i] != ',' && s[i] != '>')) return false;
  i++;
  skip_ws(i);
  if (!read_int(i, b)) return false;
  skip_ws(i);
  return i == s.size();
}

int32_t check_shader_precision(const nlohmann::json& p, char** err) {
  const nlohmann::json* n = xhl::node(p, "webGl:shaderPrecisionFormats");
  if (!n) return XHL_OK;
  if (!n->is_object()) {
    return fail("field 'webGl:shaderPrecisionFormats' harus berupa objek", err);
  }
  for (auto it = n->begin(); it != n->end(); ++it) {
    int64_t a = 0;
    int64_t b = 0;
    if (!parse_int_pair(it.key(), &a, &b)) {
      return fail("kunci shaderPrecisionFormats '" + it.key() +
                      "' tidak berformat '<int>,<int>'",
                  err);
    }
    const nlohmann::json& v = it.value();
    if (!v.is_object()) {
      return fail("nilai shaderPrecisionFormats '" + it.key() + "' harus berupa objek", err);
    }
    for (const char* field : {"rangeMin", "rangeMax", "precision"}) {
      if (v.find(field) == v.end()) {
        return fail("shaderPrecisionFormats '" + it.key() + "' kehilangan field '" + field + "'",
                    err);
      }
      if (!v[field].is_number()) {
        return fail("shaderPrecisionFormats '" + it.key() + "' field '" + field +
                        "' harus berupa angka",
                    err);
      }
    }
  }
  return XHL_OK;
}

// Entri voices wajib lengkap lima field (pola MVoices camoufox): entri separuh
// adalah entri yang tidak dapat dipakai, jadi lebih baik dilaporkan.
int32_t check_voices(const nlohmann::json& p, char** err) {
  const nlohmann::json* n = xhl::node(p, "voices");
  if (!n) return XHL_OK;
  if (!n->is_array()) return fail("field 'voices' harus berupa larik", err);
  size_t idx = 0;
  for (const nlohmann::json& v : *n) {
    const std::string at = "voices[" + std::to_string(idx) + "]";
    if (!v.is_object()) return fail(at + " harus berupa objek", err);
    for (const char* field : {"lang", "name", "voiceUri"}) {
      auto val = xhl::get_str(v, field);
      if (!val) return fail(at + " kehilangan field teks '" + field + "'", err);
    }
    for (const char* field : {"isDefault", "isLocalService"}) {
      if (!xhl::node(v, field)) return fail(at + " kehilangan field '" + field + "'", err);
      auto b = xhl::get_bool(v, field);
      if (!b) return fail(at + " field '" + field + "' harus boolean", err);
    }
    idx++;
  }
  return XHL_OK;
}

int32_t check_string_array(const nlohmann::json& p, const char* key, char** err) {
  const nlohmann::json* n = xhl::node(p, key);
  if (!n) return XHL_OK;
  if (!n->is_array()) return fail(std::string("field '") + key + "' harus berupa larik", err);
  for (const nlohmann::json& item : *n) {
    if (!item.is_string()) {
      return fail(std::string("field '") + key + "' harus larik teks", err);
    }
  }
  return XHL_OK;
}

int32_t validate_impl(const nlohmann::json& p, char** err) {
  if (!p.is_object()) return fail("profil harus berupa objek JSON", err);

  for (const char* k : kUintKeys) {
    const int32_t rc = check_uint(p, k, err);
    if (rc != XHL_OK) return rc;
  }
  for (const char* k : kNumberKeys) {
    const int32_t rc = check_number(p, k, err);
    if (rc != XHL_OK) return rc;
  }
  for (const char* k : kStringKeys) {
    const int32_t rc = check_string(p, k, err);
    if (rc != XHL_OK) return rc;
  }

  // avail* hanya bermakna berpasangan (pola GetRect): satu saja berarti
  // pemanggil bermaksud menyetel keduanya.
  const bool has_aw = xhl::has(p, "screen.availWidth");
  const bool has_ah = xhl::has(p, "screen.availHeight");
  if (has_aw != has_ah) {
    return fail(std::string("screen.availWidth dan screen.availHeight harus ada bersamaan; "
                            "yang hilang: ") +
                    (has_aw ? "screen.availHeight" : "screen.availWidth"),
                err);
  }

  {
    const int32_t rc = check_shader_precision(p, err);
    if (rc != XHL_OK) return rc;
  }
  {
    const int32_t rc = check_voices(p, err);
    if (rc != XHL_OK) return rc;
  }
  {
    const int32_t rc = check_string_array(p, "fonts.list", err);
    if (rc != XHL_OK) return rc;
  }
  {
    const int32_t rc = check_string_array(p, "headers.order", err);
    if (rc != XHL_OK) return rc;
  }

  const nlohmann::json* params = xhl::node(p, "webGl:parameters");
  if (params && !params->is_object()) {
    return fail("field 'webGl:parameters' harus berupa objek", err);
  }
  return XHL_OK;
}

} // namespace

extern "C" {

int32_t xhl_profile_validate(const char* json, char** err) {
  if (!json) {
    xhl::set_err(err, "xhl_profile_validate: argumen null");
    return XHL_INVALID;
  }
  if (!nlohmann::json::accept(json)) {
    xhl::set_err(err, "profil bukan JSON yang sah");
    return XHL_PARSE;
  }
  const nlohmann::json p = nlohmann::json::parse(json, nullptr, false);
  if (p.is_discarded()) {
    xhl::set_err(err, "profil bukan JSON yang sah");
    return XHL_PARSE;
  }
  return validate_impl(p, err);
}

int32_t xhl_profile_from_json(const char* json, char** err) {
  if (!json) {
    xhl::set_err(err, "xhl_profile_from_json: argumen null");
    return XHL_INVALID;
  }
  if (!nlohmann::json::accept(json)) {
    xhl::set_err(err, "profil bukan JSON yang sah");
    return XHL_PARSE;
  }
  nlohmann::json p = nlohmann::json::parse(json, nullptr, false);
  if (p.is_discarded()) {
    xhl::set_err(err, "profil bukan JSON yang sah");
    return XHL_PARSE;
  }
  const int32_t rc = validate_impl(p, err);
  if (rc != XHL_OK) return rc;
  ProfileState& s = state();
  s.json = std::move(p);
  s.loaded = true;
  return XHL_OK;
}

void xhl_profile_clear(void) {
  ProfileState& s = state();
  s.json = nlohmann::json::object();
  s.loaded = false;
}

int32_t xhl_profile_dump(char** out_json, char** err) {
  if (!out_json) {
    xhl::set_err(err, "xhl_profile_dump: argumen null");
    return XHL_INVALID;
  }
  *out_json = nullptr;
  ProfileState& s = state();
  if (!s.loaded) {
    xhl::set_err(err, "profil belum dimuat");
    return XHL_STATE;
  }
  char* p = xhl::dup(s.json.dump(2));
  if (!p) {
    xhl::set_err(err, "alokasi gagal untuk dump profil");
    return XHL_ALLOC;
  }
  *out_json = p;
  return XHL_OK;
}

int32_t xhl_profile_loaded(void) { return state().loaded ? 1 : 0; }

} // extern "C"