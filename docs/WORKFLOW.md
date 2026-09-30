# X Headless — Development Workflow

Companion dari `docs/SYSTEM_DESIGN.md`. Mengatur cara kerja harian: setup, konvensi,
siklus fitur, testing, debugging anti-bot, release, dan runbook operasional.

---

## 1. Environment

| Kebutuhan | Versi | Catatan |
|---|---|---|
| Rust | stable terbaru (`rustup`) | `rust-toolchain.toml` mengunci channel |
| Akses jaringan ke `x.com` | — | endpoint internal; sebagian lingkungan kantor/proxy akan memblokir |
| Akun X + browser (untuk **impor cookie saja**) | — | Chromium **tidak** dibutuhkan saat runtime |
| SQLite | via `rusqlite` feature `bundled` | tidak perlu install OS-level |

Setup sekali:

```bash
rustup show
cargo install cargo-nextest cargo-deny cargo-audit git-cliff
cargo build --workspace
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

Environment variables:

| Var | Fungsi |
|---|---|
| `XHL_ACCOUNT` | akun aktif (default `default`) |
| `XHL_DATA_DIR` | override `~/.local/share/xhl` |
| `XHL_LOG` | filter `tracing` (mis. `debug`, `xhl_core::http=trace`) |
| `XHL_QUERY_IDS` | path JSON override queryId (darurat, tanpa rilis) |
| `XHL_HTTP_PROFILE` | pilih profil impersonation (`firefox_133`, `chrome_131`, …) |
| `XHL_INSECURE_TLS` | **hanya** untuk diagnosis; mematikan verifikasi TLS |
| `OPENAI_BASE_URL` / `OPENAI_API_KEY` | provider LLM (opsional) |

> Cookie **tidak** diambil dari env (hindari bocor ke `ps`/shell history). Selalu lewat `xhl auth import`.

---

## 2. Dependency Management

- Tambah dependency via `cargo add <crate>` — jangan mengetik versi dari ingatan.
- Setiap dependency baru lewat `cargo deny check` (lisensi + advisory + duplikat).
- Arah dependensi: `xhl-cli`, `xhl-mcp` → `xhl-core`. **Tidak boleh** sebaliknya.
- Khusus `rquest` vs `reqwest`: keduanya membawa TLS stack besar. Jangan pernah keduanya di
  satu binary kecuali ada alasan tertulis; pilih lewat feature flag (`impersonate` on/off).
- Crate `x-client-transaction` **tidak** di-depend; algoritmanya di-vendor (§3.4).

---

## 3. Konvensi Kode

### 3.1 Umum

- `cargo fmt` default; `cargo clippy --workspace --all-targets -- -D warnings`.
- Tidak ada `#[allow]` tanpa komentar alasan.
- Async: tokio. **Tidak ada blocking** di jalur async — I/O SQLite lewat `spawn_blocking`;
  operasi CPU-heavy (signer) boleh `spawn_blocking` bila >10 µs.
- Error: `thiserror` di library, `Result<T, XhlError>`; `anyhow` hanya di `main` adapter.
- ID domain sebagai newtype (`TweetId`, `AccountId`), bukan `String` telanjang.
- API internal X = "vendor surface": semua pengetahuan tentang bentuknya terkurung di
  `driver/graphql/` + `query/` + `antibot/`. Service layer tidak tahu nama operasi GraphQL.

### 3.2 Header & kredensial (aturan keras)

- **Satu** sumber header: `xhl-core/src/http/headers.rs`. Tidak ada request yang menyusun header sendiri.
- **Cookie tidak boleh** muncul di: `Debug` impl, log/tracing, pesan error, output CLI/MCP.
  Semua tipe yang memuat cookie MUST punya `Debug` manual yang meredaksi.
- Test wajib: ada unit test yang memastikan `format!("{:?}", account)` tidak memuat nilai cookie.
- `user-agent` dan profil TLS harus berasal dari **satu deskripsi profil**; jangan set salah satu manual.

### 3.3 QueryId & payload

- `queryId` hanya dari `QueryRegistry`. Tidak ada literal queryId di kode fitur.
- Objek `features`/`fieldToggles` GraphQL disimpan sebagai konstanta berversi
  (`graphql/features.rs`) dengan komentar tanggal & sumber penemuan.
- DTO respons memakai `serde` dan **toleran**: semua field opsional yang tidak dipakai domain
  di-`#[serde(default)]`, karena X sering menambah/menghapus field.
- Dekoder tidak boleh `unwrap()` pada bentuk respons X; gunakan `Option`/`Result`.

### 3.4 Transaction signer (vendored)

- Lokasi: `xhl-core/src/antibot/` — `cubic_curve.rs`, `interpolate.rs`, `rotation.rs`, `transaction.rs`.
- Setiap fungsi ber-paritas dengan implementasi rujukan (komentar menyebut sumber + tanggal).
- Unit test memakai **vektor rujukan**: contoh `(method, path, state) → expected signature` dari implementasi publik, supaya perubahan algoritma terdeteksi.

### 3.5 Commit

Conventional Commits:

```
feat(antibot): vendor transaction signer dengan vektor rujukan
feat(driver): tambah SearchTimeline + dekoder tweet
fix(http): csrf header ikut ct0 setelah re-import cookie
test(store): kasus jobs stale reset
docs(design): perjelas strategi queryId
```

---

## 4. Siklus Kerja Fitur

```
1. Acceptance criteria di issue (observable, bukan "implementasi X")
2. Desain singkat: operasi GraphQL, tipe domain, bentuk respons, error path
3. Implement di xhl-core + FakeDriver path
4. Fixture respons nyata (disanitasi) → test dekoder offline
5. Wire ke CLI dan/atau MCP adapter
6. Smoke test live (manual-gated) — §5.4
7. Dokumentasi: CLI help / deskripsi tool MCP
8. PR → review → squash merge
```

**Definition of Done**:

- Acceptance criteria dibuktikan (command + output di deskripsi PR).
- `cargo test --workspace` & `cargo clippy -D warnings` bersih.
- Error path tercakup minimal: `AuthExpired`, `Forbidden{Csrf|TransactionId}`, `QueryIdStale`, `RateLimited`, `UnknownOutcome` (khusus write).
- Decoder diuji terhadap fixture respons nyata.
- Tidak ada cookie di log/error; tidak ada queryId literal; tidak ada header builder ad-hoc.

---

## 5. Testing

### 5.1 Layer

| Layer | Alat | Sifat |
|---|---|---|
| Domain/service | `cargo test` + `FakeDriver` | wajib, offline |
| Decoder GraphQL | **fixture JSON nyata** (disanitasi) | wajib; ini jaring pengaman utama |
| Header builder & signer | unit + vektor rujukan | wajib |
| Store | SQLite tempdir | wajib |
| Adapter MCP | in-memory transport rmcp + fake driver | wajib untuk mapping tool |
| CLI | `assert_cmd` + fake driver | parsing & output |
| Live (real X) | binary nyata + cookie nyata | **manual-gated**, tidak di CI default |

### 5.2 Fixture & record/replay

Karena target adalah API yang berubah-ubah, fixture adalah aset proyek:

- `crates/xhl-core/tests/fixtures/<operation>/<kasus>.json` — respons asli yang **disanitasi**.
- Sanitasi: ganti `auth_token`, `ct0`, nama akun, ID, dan media URL ke nilai dummy. Ada skrip
  `xtask sanitize-fixture` yang menegakkan ini.
- **Jangan** pernah commit respons dengan data akun nyata.
- Test decoder harus mencakup: field hilang, `null`, tipe berubah (mis. `views` string vs number),
  respons error X (`errors[]`), entri `TweetWithVisibilityResults` dibungkus vs tidak.

### 5.3 Aturan test

- Uji perilaku yang bisa diamati, bukan implementasi.
- Kasus yang layak diuji: decode tweet yang `note_tweet` panjang, `SearchTimeline` dengan cursor,
  `CreateTweet` sukses/gagal, `429` header parsing, `403` reason classification,
  dedup job, dan `Running` stale reset.
- Test live diberi `#[ignore]` + gate `XHL_LIVE=1`; tidak dijalankan di CI default.
- Uji regresi anti-bot: satu test yang menandai perubahan tak terduga pada bentuk bundle
  (jumlah/hash pola) sebagai peringatan, bukan kegagalan keras.

### 5.4 Smoke test live (wajib sebelum merge untuk fitur baru)

```bash
xhl auth import --from cookies.txt     # sekali, atau saat rotasi
xhl doctor                             # semua komponen harus hijau
xhl user --handle <handle-uji>         # read path
xhl search "from:me smoke" --limit 5
xhl post "smoke test <timestamp>"      # write path → harus mengembalikan id nyata
xhl metrics <id-dari-post>
```

Catat id/URL di deskripsi PR. Jalur MCP:

```bash
RUST_LOG=debug xhl-mcp --account default
# agent/klien: tools/list → xhl_session_status → xhl_search
```

---

## 6. Debugging Anti-Bot

`xhl doctor` adalah pintu pertama. Gunakan matriks triage:

| Gejala | Kemungkinan penyebab | Tindakan |
|---|---|---|
| `401` / `AuthExpired` | cookie kedaluwarsa | `xhl auth import` ulang dari browser |
| `403 Bad CSRF Token` | `ct0` cookie ≠ header `x-csrf-token` | cek `http/headers.rs`; re-import cookie |
| `403` pada GraphQL, `/1.1/` OK | transaction-id salah/basi | refresh `TransactionState`; cek vektor rujukan signer |
| `403` di semua endpoint, termasuk `/1.1/` | fingerprint TLS / UA mismatch | cek profil `XHL_HTTP_PROFILE` vs `user-agent`; uji `XHL_INSECURE_TLS` untuk memisahkan masalah TLS |
| `403` berkepanjangan setelah posting | sesi ditandai bot / shadowban | **stop** write otomatis; jeda; pertimbangkan impor sesi baru |
| `404` pada GraphQL | queryId usang | `xhl doctor --refresh-queries`; perbarui `known_queries.json` |
| `429` | rate limit | hormati `x-rate-limit-reset`; jangan retry langsung |
| `UnknownOutcome` | respons hilang setelah kirim | verifikasi manual via `xhl search "from:me ..."`; **jangan** retry buta |
| Bootstrap signer gagal | struktur halaman/bundle X berubah | perbarui parser bundle; `xhl doctor` harus gagal jelas, bukan senyap |

> **Terverifikasi live 2026-09-30 — signer berlaku; tiga penyebab kegagalan nyata.**
> Bootstrap melewati seluruh rantai (halaman → `twitter-site-verification` → nama file
> `ondemand.s.<hash>a.js` → indeks byte `(W[NN],16)` → kurva animasi) dan GraphQL berjalan.
> Kegagalan yang benar-benar terjadi, semuanya sudah diperbaiki:
>
> 1. **HTTP/1.1 ditolak.** Klien tanpa feature `http2` di `reqwest` mendapat koneksi terputus
>    tanpa balasan HTTP (`000`). X hanya melayani HTTP/2 di sini. Gejala: `network: error
>    sending request` untuk semua endpoint padahal `curl` berhasil.
> 2. **Header `authorization: Bearer` merusak permintaan halaman HTML.** Halaman `x.com`
>    dengan **cookie saja** terkirim lengkap (304 KB, memuat `ondemand.s`); dengan Bearer
>    ikut dikirim, Cloudflare menolak koneksi. Karena itu bootstrap halaman dan discovery
>    memakai `TransactionSigner::page_headers_for` (cookie + UA + accept), **bukan**
>    `headers::build` yang menambahkan Bearer.
> 3. **Halaman anonim tidak berisi apa yang dibutuhkan.** Tanpa cookie, `x.com` tidak memuat
>    `ondemand.s` dan tidak memuat daftar script berisi operasi GraphQL. Halaman logged-in
>    memuat keduanya.
>
> Perubahan bentuk respons X yang ikut ditemukan (semua sudah ditangani + ada uji regresi):
> identitas user pindah dari `legacy` ke `core`, bio ke `profile_bio.description`,
> followers ke `relationship_counts.followers`, verifikasi ke `verification.verified`;
> timeline home memakai operasi **`TVHomeMixer`** dengan `timeline_type`
> (`ForYou`/`Following`) — `HomeTimeline`/`HomeLatestTimeline` sudah tidak ada di bundle.

### Pelajaran penting dari verifikasi live (2026-09-30)

Tiga jebakan yang semuanya sudah ditangani di kode; jangan diulang:

1. **`queryId` di `assets/known_queries.json` cepat basi.** Saat diverifikasi, seluruh hash
   di aset sudah tidak berlaku (`SearchTimeline` pun bergeser); memakai hash basi membuat X
   menjawab `GraphQL error (code None): Internal server error` — bukan 404, sehingga sulit
   dikenali. Karena itu precedence `QueryRegistry::resolve` adalah
   **discovery/cache DB → aset**: aset hanya titik awal sebelum discovery pertama.
   Jalankan `xhl queries refresh` setelah memperbarui aset atau saat GraphQL mulai aneh.
2. **Sarang timeline berbeda per operasi.** `UserTweets` dan `TweetDetail` memakai
   `timeline.timeline.instructions`; kode lama hanya membaca `timeline_v2` sehingga
   timeline user selalu kosong (tanpa error). Gunakan `decode::instructions_of()` untuk
   semua jalur.
3. **Trends tidak berada di `content.itemContent`** melainkan di
   `content.items[].item.itemContent` dengan `__typename == "TimelineTrend"`. Konteks ramai
   ada di `social_context.text` ("Trending now · Sports · 54 posts"), dan `trend_url` dikirim
   sebagai deep-link `twitter://trending/<id>` — bukan URL web.

Catatan operasional: sebagian timeline trend milik X menjawab `Internal server error` untuk
akun tertentu (teramati pada kategori `trending`/`news`/`entertainment`, sementara `sport`
normal). Itu berasal dari X, bukan bug kita; `xhl trends` melaporkannya sebagai error apa
adanya, bukan daftar palsu.

Artefak debug: dengan `XHL_LOG=debug`, simpan request/response **yang sudah diredaksi**
(path, status, header rate-limit) ke `~/.local/share/xhl/debug/`. Body respons disimpan hanya
bila `XHL_DUMP_BODY=1` dan selalu disanitasi sebelum dibagikan.

---

## 7. CI (GitHub Actions)

```
fmt        → cargo fmt --check
clippy     → cargo clippy --workspace --all-targets -- -D warnings
test       → cargo nextest run --workspace          (tanpa test live)
deny       → cargo deny check
audit      → cargo audit
build      → cargo build --release (xhl, xhl-mcp)
```

Test live tidak di CI default (butuh cookie; jangan pernah menyimpan cookie akun nyata di secret CI
untuk tujuan ini). Sebagai gantinya:

- **Nightly job** dijalankan pada runner yang menunjuk akun uji terpisah, dengan secret terenkripsi,
  dan **hanya** menjalankan read path + `xhl doctor`. Hasilnya boleh gagal tanpa memblok merge
  (sifatnya deteksi kerapuhan X).

---

## 8. Runbook Operasional

### 8.1 Rotasi cookie

Cookie `auth_token` hilang bila: logout di browser, ganti password, sesi kedaluwarsa, atau
X memaksa verifikasi. Prosedur:

1. Login akun di browser, ekspor cookie untuk domain `.x.com`.
2. `xhl auth import --from ~/Downloads/cookies.txt`
3. `xhl auth status` → harus `Ok { screen_name }`.

### 8.2 Sesi ditandai (403 persisten)

1. Hentikan scheduler (`xhl run` stop) — jangan menambah tegangan ke akun.
2. Konfirmasi cookie valid via `xhl auth status` (masih `Ok` → berarti masalah fingerprint/behavior).
3. Turunkan budget limiter (konfigurasi), tunggu beberapa jam.
4. Verifikasi akun bisa posting manual di browser.
5. Baru aktifkan kembali scheduler dengan laju lebih rendah.

### 8.3 QueryId usang massal (X update bundle)

1. `xhl doctor --refresh-queries` → registry di-update dari bundle.
2. Jika discovery gagal: perbarui `known_queries.json` dari rujukan komunitas, rilis patch.
3. Darurat tanpa rilis: `XHL_QUERY_IDS=/path/override.json`.

---

## 9. Release

- Semver tunggal untuk workspace (`[workspace.package] version`).
- Changelog dari Conventional Commits (`git-cliff`).
- Artefak: `xhl`, `xhl-mcp` (Linux x86_64 dulu), plus `docs/`.
- Sebelum rilis: jalankan smoke test §5.4 pada kandidat + `xhl doctor`.
- **Bundle `known_queries.json` diperbarui pada setiap rilis** (bagian dari artefak).
- Tag `vX.Y.Z`, rilis GitHub dengan changelog + catatan kompatibilitas X.

---

## 10. Checklist PR (reviewer)

- [ ] Arah dependensi benar (`core` tidak menarik `clap`/`rmcp`).
- [ ] Tidak ada cookie di log/`Debug`/error/output tool (ada test-nya).
- [ ] Header disusun lewat `http/headers.rs`; tidak ada literal queryId.
- [ ] Signer/parser bundle punya vektor rujukan & test paritas.
- [ ] Decoder diuji dengan fixture nyata yang sudah disanitasi.
- [ ] Tidak ada `blocking` di async; tidak ada `unwrap()` pada bentuk respons X.
- [ ] Error path baru dipetakan (retryable / manual / aman-untuk-agent).
- [ ] Write path idempoten atau punya `dedup_key`.
- [ ] Bukti acceptance criteria (command + output) ada di deskripsi PR.

---

## 11. Backlog perbaikan berkelanjutan

- Auto-discovery queryId yang lebih tahan perubahan (parsing AST-ish, bukan regex rapuh).
- `SecretStore` keyring/SQLCipher untuk cookie (ADR-006).
- `XApiDriver` sebagai jalur write legal bila biaya API dapat diterima.
- Multi-akun: aktifkan kolom `account` pada job/draft + limiter per akun.
- Telemetri internal (lokal): tingkat 403/429 per periode sebagai early-warning health akun.