// Derivasi header dari profil. Lihat antidetect.hpp.

#include "antidetect.hpp"
#include "jsonutil.hpp"

#include <string>
#include <vector>

namespace {

// Pemetaan kunci profil -> slot keluaran.
struct Field {
  const char* key;
  size_t offset; // offset xhl_headers, agar tabel ini tetap sederhana
};

// Salin bila ada; biarkan nullptr bila tidak.
void assign(const nlohmann::json& c, const char* key, const char** slot) {
  auto v = xhl::get_str(c, key);
  if (v && !v->empty()) *slot = xhl::dup(*v);
}

const char* const kDefaultOrder[] = {
    "authorization",   "x-csrf-token",    "x-twitter-auth-type",
    "x-twitter-active-user", "x-twitter-client-language", "user-agent",
    "accept",          "accept-language", "accept-encoding",
    "content-type",    "x-client-transaction-id", "cookie",
};

} // namespace

extern "C" {

int32_t xhl_headers_from_profile(const char* profile_json, xhl_headers* out, char** err) {
  if (!profile_json || !out) {
    xhl::set_err(err, "xhl_headers_from_profile: argumen null");
    return XHL_INVALID;
  }
  std::memset(out, 0, sizeof(*out));
  if (!nlohmann::json::accept(profile_json)) {
    xhl::set_err(err, "profil bukan JSON yang sah");
    return XHL_PARSE;
  }
  const nlohmann::json c = nlohmann::json::parse(profile_json, nullptr, false);
  if (c.is_discarded() || !c.is_object()) {
    xhl::set_err(err, "profil harus berupa objek JSON");
    return XHL_INVALID;
  }

  assign(c, "headers.User-Agent", &out->user_agent);
  if (!out->user_agent) {
    assign(c, "navigator.userAgent", &out->user_agent);
    if (out->user_agent) out->from_navigator_fallback = 1;
  }
  assign(c, "headers.Accept-Language", &out->accept_language);
  assign(c, "headers.Accept-Encoding", &out->accept_encoding);
  assign(c, "headers.Accept", &out->accept);
  assign(c, "headers.DNT", &out->dnt);
  assign(c, "headers.Sec-CH-UA", &out->sec_ch_ua);
  assign(c, "headers.Sec-CH-UA-Mobile", &out->sec_ch_ua_mobile);
  assign(c, "headers.Sec-CH-UA-Platform", &out->sec_ch_ua_platform);

  auto vw = xhl::get_int(c, "headers.Viewport-Width");
  if (vw) out->viewport_width = xhl::dup(std::to_string(*vw));

  // Urutan header: dari profil bila ada, selain itu bawaan xhl.
  const nlohmann::json* order = xhl::node(c, "headers.order");
  std::vector<std::string> list;
  if (order && order->is_array()) {
    for (const nlohmann::json& item : *order) {
      if (item.is_string()) list.push_back(item.get_ref<const std::string&>());
    }
  }
  size_t n = list.empty() ? (sizeof(kDefaultOrder) / sizeof(kDefaultOrder[0])) : list.size();
  char** arr = static_cast<char**>(std::calloc(n, sizeof(char*)));
  if (!arr) {
    xhl_headers_free(out);
    xhl::set_err(err, "alokasi gagal untuk urutan header");
    return XHL_ALLOC;
  }
  for (size_t i = 0; i < n; i++) {
    const std::string& s = list.empty() ? std::string(kDefaultOrder[i]) : list[i];
    arr[i] = xhl::dup(s);
    if (!arr[i]) {
      xhl_headers_free(out);
      xhl_strings_free(arr, n);
      xhl::set_err(err, "alokasi gagal untuk elemen urutan");
      return XHL_ALLOC;
    }
  }
  out->order = const_cast<const char**>(arr);
  out->order_n = n;

  if (out->user_agent || out->accept_language || out->accept_encoding || out->accept ||
      out->dnt || out->viewport_width || out->sec_ch_ua || out->sec_ch_ua_mobile ||
      out->sec_ch_ua_platform || !list.empty()) {
    out->from_profile = 1;
  }
  return XHL_OK;
}

void xhl_headers_free(xhl_headers* h) {
  if (!h) return;
  std::free(const_cast<char*>(h->user_agent));
  std::free(const_cast<char*>(h->accept_language));
  std::free(const_cast<char*>(h->accept_encoding));
  std::free(const_cast<char*>(h->accept));
  std::free(const_cast<char*>(h->dnt));
  std::free(const_cast<char*>(h->viewport_width));
  std::free(const_cast<char*>(h->sec_ch_ua));
  std::free(const_cast<char*>(h->sec_ch_ua_mobile));
  std::free(const_cast<char*>(h->sec_ch_ua_platform));
  if (h->order) xhl::free_strings(const_cast<char**>(h->order), h->order_n);
  std::memset(h, 0, sizeof(*h));
}

} // extern "C"