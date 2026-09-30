// Aturan koherensi identitas. Diport dari camoufox/pythonlib/camoufox/coherence.py.
// Lihat coherence.hpp untuk alasan fase ini ada.

#include "coherence.hpp"
#include "jsonutil.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <set>
#include <string>
#include <vector>

namespace {

using json = nlohmann::json;

// Target OS.
constexpr const char* kWin = "win";
constexpr const char* kMac = "mac";
constexpr const char* kLin = "lin";

// --- Konstanta, disalin dari coherence.py (baris 39-113). ---

// Jumlah core yang benar-benar dikirim Apple Silicon: M1 adalah lantai di 8.
// 11 ada karena M3 Pro 6P+5E; 9, 13, 15 bukan bagian Apple.
const std::set<int64_t> kAppleSiliconCores = {8, 10, 11, 12, 14, 16, 20, 24, 28, 32};

// devicePixelRatio per platform. Tuple menaik, bukan himpunan: perbaikan
// mengambil langkah pertama dari dua yang sama dekat.
const std::vector<double> kDprWin = {1, 1.25, 1.5, 1.75, 2, 2.5, 3};
const std::vector<double> kDprMac = {1, 2};
const std::vector<double> kDprLin = {1, 1.25, 1.5, 1.75, 2};

// Firefox melaporkan 24, atau 30 di layar warna dalam.
const std::set<int64_t> kPlausibleColorDepth = {24, 30};

constexpr int64_t kMaxPlausibleTouchPoints = 10;

// Tinggi chrome browser: strip toolbar di atas area konten. Sifat *binary*,
// bukan identitas. inner + 86 tidak boleh melebihi avail.
constexpr int64_t kBrowserChromeHeight = 86;

// String GPU yang mustahil di macOS (Firefox merender via CGL, bukan ANGLE).
const char* const kNotAMacGpu[] = {"ANGLE", "llvmpipe"};

const std::set<int64_t> kIntelMacIgpCores = {2, 4, 6, 8, 12, 16};
const std::set<int64_t> kIntelMacDgpuCores = {2,  4,  6,  8,  10, 12, 14, 16,
                                              18, 20, 24, 28, 32, 36, 48, 56};

// Panel MacBook ber-notch + iMac 24": hanya ada di Apple Silicon.
const std::pair<int64_t, int64_t> kAppleSiliconPanels[] = {
    {1024, 665}, {1280, 832}, {1470, 956}, {1710, 1112}, // Air 13.6"
    {1280, 828}, {1440, 932}, {1710, 1107},              // Air 15.3"
    {1147, 745}, {1352, 878}, {1512, 982}, {1800, 1169}, // Pro 14"
    {1312, 848}, {1496, 967}, {1728, 1117}, {2056, 1329}, // Pro 16"
    {2240, 1260},                                         // iMac 24"
};

const std::vector<double>* dpr_for(const std::string& os) {
  if (os == kWin) return &kDprWin;
  if (os == kMac) return &kDprMac;
  if (os == kLin) return &kDprLin;
  return nullptr;
}

// --- Pembantu ---

std::string renderer_of(const json& c) {
  auto v = xhl::get_str(c, "webGl:renderer");
  return v ? *v : std::string();
}

bool is_apple_silicon(const json& c) { return renderer_of(c).find("Apple M") != std::string::npos; }

bool contains(const std::string& hay, const char* needle) {
  return hay.find(needle) != std::string::npos;
}

// Format detail: "<prefix><value><suffix>" dalam satu tempat agar pesan seragam.
std::string num_join(const char* a, int64_t v, const char* b) {
  return std::string(a) + std::to_string(v) + b;
}

bool gpu_fits_os(const std::string& renderer, const std::string& os) {
  if (renderer.empty()) return true;
  if (os == kMac) {
    for (const char* bad : kNotAMacGpu) {
      if (contains(renderer, bad)) return false;
    }
    return true;
  }
  if (os == kWin) return renderer.rfind("ANGLE", 0) == 0;
  if (os == kLin) return !contains(renderer, "ANGLE") && !contains(renderer, "Apple M");
  return true;
}

// Mengapa tidak ada Intel Mac yang melaporkan GPU ini bersama core/layar ini.
bool intel_mac_misfit(const std::string& renderer, const json& c, std::string* why) {
  if (renderer.empty() || contains(renderer, "Apple M")) return false;
  const std::set<int64_t>& allowed =
      contains(renderer, "Intel") ? kIntelMacIgpCores : kIntelMacDgpuCores;
  auto cores = xhl::get_int(c, "navigator.hardwareConcurrency");
  if (cores && allowed.find(*cores) == allowed.end()) {
    if (why) {
      *why = "renderer '" + renderer + "' dengan hardwareConcurrency " + std::to_string(*cores) +
             "; tidak ada Intel Mac dengan GPU itu yang melaporkannya";
    }
    return true;
  }
  auto w = xhl::get_int(c, "screen.width");
  auto h = xhl::get_int(c, "screen.height");
  if (w && h) {
    for (const auto& p : kAppleSiliconPanels) {
      if (p.first == *w && p.second == *h) {
        if (why) {
          *why = "renderer '" + renderer + "' di balik panel " + std::to_string(*w) + "x" +
                 std::to_string(*h) + ", yang hanya dimiliki Apple Silicon Mac";
        }
        return true;
      }
    }
  }
  return false;
}

// --- Pemeriksaan. Mengembalikan detail bila aturan dilanggar. ---

bool check_apple_silicon_cores(const json& c, const std::string&, std::string* detail) {
  if (!is_apple_silicon(c)) return false;
  auto cores = xhl::get_int(c, "navigator.hardwareConcurrency");
  if (cores && kAppleSiliconCores.find(*cores) == kAppleSiliconCores.end()) {
    if (detail) {
      *detail = "'" + renderer_of(c) + "' dengan hardwareConcurrency " +
                std::to_string(*cores) + "; Apple Silicon mulai dari 8";
    }
    return true;
  }
  return false;
}

// Perbaikan: naik ke jumlah core terdekat yang dikirim Apple.
void repair_apple_silicon_cores(json& c, const std::string&) {
  auto cores = xhl::get_int(c, "navigator.hardwareConcurrency");
  if (!cores) return;
  int64_t chosen = *kAppleSiliconCores.rbegin();
  for (int64_t v : kAppleSiliconCores) {
    if (v >= *cores) {
      chosen = v;
      break;
    }
  }
  xhl::set(c, "navigator.hardwareConcurrency", chosen);
}

bool check_gpu_matches_os(const json& c, const std::string& os, std::string* detail) {
  const std::string r = renderer_of(c);
  if (r.empty() || gpu_fits_os(r, os)) return false;
  if (detail) {
    if (os == kMac) {
      *detail = "identitas macOS dengan '" + r + "', yang tidak dilaporkan Mac mana pun";
    } else if (os == kWin) {
      *detail = "identitas Windows dengan '" + r + "'; Firefox di Windows merender lewat ANGLE";
    } else {
      *detail = "identitas Linux dengan '" + r + "'";
    }
  }
  return true;
}

bool check_intel_mac_hardware(const json& c, const std::string& os, std::string* detail) {
  if (os != kMac) return false;
  std::string why;
  if (intel_mac_misfit(renderer_of(c), c, &why)) {
    if (detail) *detail = why;
    return true;
  }
  return false;
}

bool check_color_depth(const json& c, const std::string& os, std::string* detail) {
  auto depth = xhl::get_int(c, "screen.colorDepth");
  if (!depth) return false;
  if (kPlausibleColorDepth.find(*depth) == kPlausibleColorDepth.end()) {
    if (detail) *detail = num_join("screen.colorDepth ", *depth, "; Firefox melaporkan 24 atau 30");
    return true;
  }
  if (os == kMac && is_apple_silicon(c) && *depth != 30) {
    if (detail) {
      *detail = num_join("Apple Silicon Mac dengan colorDepth ", *depth,
                         "; warna dalam adalah bawaan macOS");
    }
    return true;
  }
  return false;
}

void repair_color_depth(json& c, const std::string& os) {
  auto depth = xhl::get_int(c, "screen.colorDepth");
  if (depth && kPlausibleColorDepth.find(*depth) == kPlausibleColorDepth.end()) {
    xhl::set(c, "screen.colorDepth", 24);
  }
  if (os == kMac && is_apple_silicon(c)) xhl::set(c, "screen.colorDepth", 30);

  // pixelDepth adalah angka yang sama di setiap browser yang melaporkan keduanya.
  if (xhl::has(c, "screen.pixelDepth") || xhl::has(c, "screen.colorDepth")) {
    auto d = xhl::get_int(c, "screen.colorDepth");
    xhl::set(c, "screen.pixelDepth", d ? *d : 24);
  }
}

bool check_touch_points(const json& c, const std::string& os, std::string* detail) {
  const json* n = xhl::node(c, "navigator.maxTouchPoints");
  if (!n) return false;
  auto touch = xhl::get_int(c, "navigator.maxTouchPoints");
  if (!touch) {
    if (detail) *detail = "navigator.maxTouchPoints harus bilangan bulat";
    return true;
  }
  if (*touch < 0 || *touch > kMaxPlausibleTouchPoints) {
    if (detail) {
      *detail = num_join("navigator.maxTouchPoints ", *touch,
                         "; digitizer melaporkan paling banyak 10");
    }
    return true;
  }
  if (os == kMac && *touch != 0) {
    if (detail) {
      *detail = num_join("identitas macOS dengan maxTouchPoints ", *touch,
                         "; tidak ada Mac berlaya sentuh");
    }
    return true;
  }
  return false;
}

void repair_touch_points(json& c, const std::string& os) {
  const json* n = xhl::node(c, "navigator.maxTouchPoints");
  if (!n) return;
  auto touch = xhl::get_int(c, "navigator.maxTouchPoints");
  const bool too_many = touch && *touch > kMaxPlausibleTouchPoints;
  // Mesin yang mengaku 256 kontak bukan mesin dengan layar sentuh lebih baik;
  // nilainya derau, jadi identitas tidak punya digitizer.
  if (os == kMac || too_many || !touch) xhl::set(c, "navigator.maxTouchPoints", 0);
}

bool check_device_pixel_ratio(const json& c, const std::string& os, std::string* detail) {
  auto dpr = xhl::get_double(c, "window.devicePixelRatio");
  if (!dpr) return false;
  const std::vector<double>* allowed = dpr_for(os);
  if (!allowed) return false;
  const bool ok = std::any_of(allowed->begin(), allowed->end(),
                              [&](double v) { return std::fabs(v - *dpr) < 1e-9; });
  if (!ok) {
    if (detail) {
      *detail = "window.devicePixelRatio " + std::to_string(*dpr) +
                " bukan mode tampilan yang ditawarkan " + os;
    }
    return true;
  }
  return false;
}

void repair_device_pixel_ratio(json& c, const std::string& os) {
  auto dpr = xhl::get_double(c, "window.devicePixelRatio");
  const std::vector<double>* allowed = dpr_for(os);
  if (!dpr || !allowed || allowed->empty()) return;
  // Langkah skala nyata terdekat: 1.818 menjadi 1.75 di Windows, 2 di macOS.
  double best = (*allowed)[0];
  double best_d = std::fabs(best - *dpr);
  for (double v : *allowed) {
    const double d = std::fabs(v - *dpr);
    if (d < best_d) {
      best_d = d;
      best = v;
    }
  }
  xhl::set(c, "window.devicePixelRatio", best);
}

bool check_window_chrome(const json& c, const std::string&, std::string* detail) {
  auto inner = xhl::get_int(c, "window.innerHeight");
  auto outer = xhl::get_int(c, "window.outerHeight");
  if (!inner || !outer || *inner == 0 || *outer == 0) return false;
  const int64_t diff = *outer - *inner;
  if (diff < kBrowserChromeHeight) {
    if (detail) {
      *detail = "window.outerHeight " + std::to_string(*outer) + " - innerHeight " +
                std::to_string(*inner) + " = " + std::to_string(diff) + ", kurang dari " +
                std::to_string(kBrowserChromeHeight) +
                "px chrome yang benar-benar dimiliki jendela; bagian bawah viewport tidak dapat "
                "menerima input";
    }
    return true;
  }
  return false;
}

void repair_window_chrome(json& c, const std::string&) {
  auto inner = xhl::get_int(c, "window.innerHeight");
  auto outer = xhl::get_int(c, "window.outerHeight");
  if (!inner || !outer || *inner == 0 || *outer == 0) return;
  auto avail = xhl::get_int(c, "screen.availHeight");
  if (!avail || *avail == 0) avail = xhl::get_int(c, "screen.height");
  // Utamakan memperbesar jendela, yang mempertahankan viewport yang digambar.
  if (!avail || *inner + kBrowserChromeHeight <= *avail) {
    xhl::set(c, "window.outerHeight", *inner + kBrowserChromeHeight);
    return;
  }
  // Tidak ada ruang di layar yang diklaim: kecilkan viewport.
  xhl::set(c, "window.innerHeight", std::max<int64_t>(*outer - kBrowserChromeHeight, 1));
}

bool check_screen_shape(const json& c, const std::string&, std::string* detail) {
  auto w = xhl::get_int(c, "screen.width");
  auto h = xhl::get_int(c, "screen.height");
  if (!w || !h || *w == 0 || *h == 0) return false;
  if (*h > *w) {
    if (detail) {
      *detail = "layar portrait " + std::to_string(*w) + "x" + std::to_string(*h) +
                "; panel desktop itu landscape";
    }
    return true;
  }
  if (*w < 1024) {
    if (detail) {
      *detail = "layar " + std::to_string(*w) + "x" + std::to_string(*h) +
                " lebih kecil dari panel desktop mana pun saat ini";
    }
    return true;
  }
  return false;
}

bool check_avail_bounds(const json& c, const std::string&, std::string* detail) {
  auto w = xhl::get_int(c, "screen.width");
  auto h = xhl::get_int(c, "screen.height");
  auto aw = xhl::get_int(c, "screen.availWidth");
  auto ah = xhl::get_int(c, "screen.availHeight");
  if (w && aw && *aw > *w) {
    if (detail) {
      *detail = num_join("screen.availWidth ", *aw, "") + " melebihi screen.width " +
                std::to_string(*w);
    }
    return true;
  }
  if (h && ah && *ah > *h) {
    if (detail) {
      *detail = num_join("screen.availHeight ", *ah, "") + " melebihi screen.height " +
                std::to_string(*h);
    }
    return true;
  }
  return false;
}

void repair_avail_bounds(json& c, const std::string&) {
  auto w = xhl::get_int(c, "screen.width");
  auto h = xhl::get_int(c, "screen.height");
  auto aw = xhl::get_int(c, "screen.availWidth");
  auto ah = xhl::get_int(c, "screen.availHeight");
  if (w && aw && *aw > *w) xhl::set(c, "screen.availWidth", *w);
  if (h && ah && *ah > *h) xhl::set(c, "screen.availHeight", *h);
}

bool check_arch_agreement(const json& c, const std::string&, std::string* detail) {
  auto ua = xhl::get_str(c, "navigator.userAgent");
  auto platform = xhl::get_str(c, "navigator.platform");
  auto oscpu = xhl::get_str(c, "navigator.oscpu");
  if (!ua || ua->empty()) return false;
  const bool arm = (platform && contains(*platform, "armv")) || (oscpu && contains(*oscpu, "armv"));
  if (contains(*ua, "x86_64") && arm) {
    if (detail) {
      *detail = "user agent mengaku x86_64 sedangkan platform/oscpu menyatakan '" +
                (platform ? *platform : std::string()) + "'/'" +
                (oscpu ? *oscpu : std::string()) + "'";
    }
    return true;
  }
  return false;
}

// --- Daftar aturan, dalam urutan yang sama dengan sumber. ---

struct Rule {
  const char* name;
  bool (*check)(const json&, const std::string&, std::string*);
  void (*repair)(json&, const std::string&); // nullptr bila tidak dapat diperbaiki
};

const Rule kRules[] = {
    {"apple-silicon-cores", check_apple_silicon_cores, repair_apple_silicon_cores},
    {"gpu-matches-os", check_gpu_matches_os, nullptr},
    {"intel-mac-hardware", check_intel_mac_hardware, nullptr},
    {"color-depth", check_color_depth, repair_color_depth},
    {"touch-points", check_touch_points, repair_touch_points},
    {"device-pixel-ratio", check_device_pixel_ratio, repair_device_pixel_ratio},
    {"window-chrome", check_window_chrome, repair_window_chrome},
    {"screen-shape", check_screen_shape, nullptr},
    {"avail-bounds", check_avail_bounds, repair_avail_bounds},
    {"arch-agreement", check_arch_agreement, nullptr},
};

struct Violation {
  std::string rule;
  std::string detail;
};

std::vector<Violation> validate_impl(const json& c, const std::string& os) {
  std::vector<Violation> out;
  for (const Rule& r : kRules) {
    std::string detail;
    if (r.check(c, os, &detail)) out.push_back({r.name, detail});
  }
  return out;
}

// Susun larik C dari daftar pelanggaran. Mengembalikan XHL_ALLOC bila gagal.
int32_t emit(const std::vector<Violation>& vs, xhl_violation** out, size_t* n, char** err) {
  *out = nullptr;
  *n = 0;
  if (vs.empty()) return XHL_OK;
  xhl_violation* arr =
      static_cast<xhl_violation*>(std::calloc(vs.size(), sizeof(xhl_violation)));
  if (!arr) {
    xhl::set_err(err, "alokasi gagal untuk daftar pelanggaran");
    return XHL_ALLOC;
  }
  for (size_t i = 0; i < vs.size(); i++) {
    arr[i].rule = xhl::dup(vs[i].rule);
    arr[i].detail = xhl::dup(vs[i].detail);
    if (!arr[i].rule || !arr[i].detail) {
      xhl_violations_free(arr, vs.size());
      xhl::set_err(err, "alokasi gagal untuk pelanggaran");
      return XHL_ALLOC;
    }
  }
  *out = arr;
  *n = vs.size();
  return XHL_OK;
}

int32_t parse_profile(const char* json_str, const char* target_os, json* out,
                      std::string* os, char** err) {
  if (!json_str) {
    xhl::set_err(err, "argumen profil null");
    return XHL_INVALID;
  }
  if (!target_os) {
    xhl::set_err(err, "target_os null");
    return XHL_INVALID;
  }
  *os = target_os;
  if (*os != kWin && *os != kMac && *os != kLin) {
    xhl::set_err(err, "target_os tidak dikenal: '" + *os + "'; pakai win, mac, atau lin");
    return XHL_INVALID;
  }
  if (!nlohmann::json::accept(json_str)) {
    xhl::set_err(err, "profil bukan JSON yang sah");
    return XHL_PARSE;
  }
  *out = nlohmann::json::parse(json_str, nullptr, false);
  if (out->is_discarded()) {
    xhl::set_err(err, "profil bukan JSON yang sah");
    return XHL_PARSE;
  }
  if (!out->is_object()) {
    xhl::set_err(err, "profil harus berupa objek JSON");
    return XHL_INVALID;
  }
  return XHL_OK;
}

} // namespace

extern "C" {

int32_t xhl_coherence_validate(const char* profile_json, const char* target_os,
                               xhl_violation** out, size_t* n, char** err) {
  if (!out || !n) {
    xhl::set_err(err, "xhl_coherence_validate: argumen null");
    return XHL_INVALID;
  }
  json c;
  std::string os;
  const int32_t rc = parse_profile(profile_json, target_os, &c, &os, err);
  if (rc != XHL_OK) return rc;
  return emit(validate_impl(c, os), out, n, err);
}

int32_t xhl_coherence_apply(const char* profile_json, const char* target_os, char** out_json,
                            xhl_violation** out, size_t* n, char** err) {
  if (!out_json || !out || !n) {
    xhl::set_err(err, "xhl_coherence_apply: argumen null");
    return XHL_INVALID;
  }
  *out_json = nullptr;
  json c;
  std::string os;
  const int32_t rc = parse_profile(profile_json, target_os, &c, &os, err);
  if (rc != XHL_OK) return rc;

  // Urutan penting: tiap perbaikan melihat keadaan terkini, sehingga perbaikan
  // berikutnya (mis. avail-bounds) memakai layar yang sudah diperbaiki.
  for (const Rule& r : kRules) {
    if (!r.repair) continue;
    std::string detail;
    if (r.check(c, os, &detail)) r.repair(c, os);
  }

  const std::vector<Violation> remaining = validate_impl(c, os);
  const int32_t erc = emit(remaining, out, n, err);
  if (erc != XHL_OK) return erc;

  char* p = xhl::dup(c.dump(2));
  if (!p) {
    xhl_violations_free(*out, *n);
    *out = nullptr;
    *n = 0;
    xhl::set_err(err, "alokasi gagal untuk JSON hasil perbaikan");
    return XHL_ALLOC;
  }
  *out_json = p;
  return XHL_OK;
}

int32_t xhl_coherence_drop_incoherent(const char* profile_json, const char* target_os,
                                      char** out_json, xhl_violation** out, size_t* n,
                                      char** err) {
  if (!out_json || !out || !n) {
    xhl::set_err(err, "xhl_coherence_drop_incoherent: argumen null");
    return XHL_INVALID;
  }
  *out_json = nullptr;
  json c;
  std::string os;
  const int32_t rc = parse_profile(profile_json, target_os, &c, &os, err);
  if (rc != XHL_OK) return rc;

  std::vector<Violation> dropped;
  const std::string r = renderer_of(c);
  if (!r.empty()) {
    const char* rule = nullptr;
    std::string why;
    if (!gpu_fits_os(r, os)) {
      rule = "gpu-matches-os";
    } else if (intel_mac_misfit(r, c, &why)) {
      rule = "intel-mac-hardware";
    }
    if (rule) {
      xhl::erase(c, "webGl:renderer");
      xhl::erase(c, "webGl:vendor");
      dropped.push_back({rule, "dibuang '" + r + "' untuk identitas " + os});
    }
  }

  const int32_t erc = emit(dropped, out, n, err);
  if (erc != XHL_OK) return erc;

  char* p = xhl::dup(c.dump(2));
  if (!p) {
    xhl_violations_free(*out, *n);
    *out = nullptr;
    *n = 0;
    xhl::set_err(err, "alokasi gagal untuk JSON hasil drop");
    return XHL_ALLOC;
  }
  *out_json = p;
  return XHL_OK;
}

void xhl_violations_free(xhl_violation* v, size_t n) {
  if (!v) return;
  for (size_t i = 0; i < n; i++) {
    std::free(const_cast<char*>(v[i].rule));
    std::free(const_cast<char*>(v[i].detail));
  }
  std::free(v);
}

} // extern "C"