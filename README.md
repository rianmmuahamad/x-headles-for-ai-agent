# X Headless (`xhl`)

Headless control plane untuk mengoperasikan akun X (Twitter) dari AI agent (Hermes, omp, Claude, dsb).

**Tanpa browser.** Auth via cookie, transport HTTP/GraphQL langsung ke endpoint internal X.

> ⚠️ **Risiko ToS**: akses endpoint internal X dengan cookie melanggar Terms of Service X dan akun
> dapat di-suspend. Batasi laju, jangan dipakai untuk spam. Lihat `docs/SYSTEM_DESIGN.md` §1.1.

## Status

M3–M9 terimplementasi. Semua perintah di bawah berjalan nyata; `--driver fake` tersedia untuk
pengembangan offline.

| Bagian | Status |
|---|---|
| Domain, error model, store SQLite (migrasi v1+v2) | ✅ |
| HTTP/GraphQL ke `x.com/i/api` (cookie + csrf) | ✅ |
| Signer `x-client-transaction-id` (vendored, uji paritas) | ✅ |
| QueryRegistry + discovery `queryId` dari bundle web | ✅ |
| Rate limiter berbasis header + penalti 429 | ✅ |
| Read: `user`, `search`, `timeline`, `thread`, `metrics` | ✅ |
| Riset ramai: `trends` (4 kategori) + `search --top` + engagement | ✅ |
| Write: `post`, thread, media (gambar + video chunked), dedup | ✅ |
| Analytics: snapshot + delta | ✅ |
| Scheduler: job, retry/backoff, dead-letter, `xhl run` | ✅ |
| Draft + provider LLM (opsional, OpenAI-compatible) | ✅ |
| MCP server (23 tool, stdio) | ✅ |
| Inti C++ native: config tervalidasi, skema profil, koherensi identitas | ✅ |
| Fingerprint jaringan: profil TLS/HTTP2 + override header dari profil | ✅ |
| `FakeDriver` (offline, untuk test) | ✅ |
| Impersonasi TLS (`--features impersonate`) | ⚠️ butuh paket sistem — lihat bawah |

Catatan penting: perintah dengan driver default (`--driver graphql`) **membutuhkan cookie yang
diimpor**. Tanpa itu ia gagal dengan pesan `AuthExpired`, bukan diam-diam memakai data palsu.

> ✅ **Diverifikasi live terhadap X dengan sesi nyata (2026-09-30).** Rantai lengkap berjalan:
> `doctor` → `session: ok`, discovery menemukan **104 operasi GraphQL**, `search`/`user`/
> `timeline` mengembalikan data nyata, dan `post` benar-benar menerbitkan tweet
> (dikonfirmasi lewat `TweetDetail`, author + teks cocok). Dedup terbukti: perintah identik
> dua kali menghasilkan ID tweet yang sama.
>
> Tiga temuan yang membuat ini bisa jalan (semuanya sudah diperbaiki):
> 1. **HTTP/2 wajib.** X memutus koneksi dari klien yang hanya berbicara HTTP/1.1; feature
>    `http2` pada `reqwest` harus aktif.
> 2. **Bootstrap signer dan discovery butuh cookie.** Halaman `x.com` versi anonim tidak
>    memuat `ondemand.s` maupun daftar operasi GraphQL. Selain itu header
>    `authorization: Bearer` **tidak boleh** dikirim ke permintaan halaman HTML — Cloudflare
>    menolak koneksinya; kirim cookie saja.
> 3. **Bentuk respons X berubah.** Identitas user kini di `core`/`profile_bio`/
>    `relationship_counts` (bukan `legacy`), timeline home memakai operasi
>    `TVHomeMixer` dengan `timeline_type` (bukan `HomeTimeline`), trend ada di
>    `content.items[].item.itemContent`, dan timeline user di
>    `timeline.timeline.instructions`.
> 4. **`queryId` di `known_queries.json` cepat basi** — memakai hash basi membuat X menjawab
>    `Internal server error`. Karena itu hasil discovery/cache selalu menang atas aset;
>    jalankan `xhl queries refresh` bila GraphQL mulai aneh.

## Pakai

> Baru pertama kali? Ikuti **`docs/MULAI.md`** — panduan langkah demi langkah dari
> build, ekspor cookie dari browser, sampai dipakai agent lewat MCP.

```bash
cargo build --workspace

# 1. Impor cookie sesi (Netscape cookies.txt, JSON, atau string `name=value; ...`)
./target/debug/xhl auth import --from cookies.txt
./target/debug/xhl auth status
./target/debug/xhl doctor          # status cookie, signer, queryId, profil

# 2. Riset (temukan yang sedang rame)
./target/debug/xhl trends --limit 20
./target/debug/xhl trends --category news
./target/debug/xhl search "rust async" --top --limit 20
./target/debug/xhl user --handle jack
./target/debug/xhl timeline --kind home --limit 20

# 3. Posting
./target/debug/xhl post "halo dari xhl"
./target/debug/xhl post "dengan media" --media gambar.png
./target/debug/xhl post "balasan" --reply-to 1234567890

# 4. Analytics
./target/debug/xhl metrics 1234567890
./target/debug/xhl metrics --history 1234567890
./target/debug/xhl metrics --delta 1234567890

# 5. Draft (& LLM)
./target/debug/xhl draft new --body "teks draft" --name peluncuran
./target/debug/xhl draft generate --prompt "tweet singkat tentang Rust async"
./target/debug/xhl draft list
./target/debug/xhl draft post <draft-id>

# 6. Penjadwalan
./target/debug/xhl schedule post --at "2026-10-01T09:00:00+07:00" --text "terjadwal"
./target/debug/xhl schedule list
./target/debug/xhl schedule cancel <job-id> --confirm
./target/debug/xhl run                # daemon (Ctrl-C berhenti)
./target/debug/xhl run --once

# 7. Diagnostik anti-bot
./target/debug/xhl queries list
./target/debug/xhl queries refresh
./target/debug/xhl debug raw --op SearchTimeline --vars '{"rawQuery":"rust","count":5,"querySource":"typed_query","product":"Latest"}'

# Pengembangan offline (data sintetis, tanpa jaringan):
./target/debug/xhl --driver fake search "rust" --limit 3
```

Variabel lingkungan:

| Var | Fungsi |
|---|---|
| `XHL_DATA_DIR` | direktori data (default `~/.local/share/xhl`) |
| `XHL_ACCOUNT` | akun aktif (default `default`) |
| `XHL_LOG` | filter log ke stderr (mis. `debug`; default `warn`) |
| `XHL_HTTP_PROFILE` | profil HTTP: `firefox_133` (default) atau `chrome_131` |
| `XHL_NO_WAIT` | `1` = jangan tunggu rate limit, kembalikan error |
| `XHL_QUERY_IDS` | path JSON untuk override `queryId` tanpa rilis |
| `OPENAI_BASE_URL` + `OPENAI_API_KEY` | aktifkan provider LLM untuk `draft generate` |
| `XHL_LLM_MODEL` | model LLM (default `gpt-4o-mini`) |

> `--driver fake` **selalu** menghasilkan data sintetis. Jangan pernah dipakai untuk mengukur
> perilaku X yang sebenarnya.

## MCP server

Agent dapat memakai `xhl-mcp` (stdio). 23 tool tersedia: `xhl_session_status`, `xhl_search`,
`xhl_timeline`, `xhl_thread_read`, `xhl_user_lookup`, `xhl_post`, `xhl_post_with_media`,
`xhl_thread_post`, `xhl_metrics`, `xhl_metrics_history`, `xhl_draft_create`, `xhl_draft_generate`,
`xhl_draft_list`, `xhl_post_from_draft`, `xhl_schedule_post`, `xhl_schedule_list`,
`xhl_schedule_cancel`, `xhl_queries_status`, `xhl_trends`, `xhl_search_ranked`,
`xhl_thread_post_from_file`, `xhl_native_info`, `xhl_profile_check`.

```json
{ "mcpServers": { "xhl": { "command": "xhl-mcp", "args": ["--account", "default"] } } }
```

- Cookie/header **tidak pernah** menjadi argumen atau hasil tool.
- Kegagalan operasional (validasi, sesi kedaluwarsa, rate limit, 403) dikembalikan in-band dengan
  `isError = true` dan pesan actionable — bukan JSON-RPC error.
- `xhl_schedule_cancel` menolak tanpa `confirm: true`.
- Logging menuju **stderr**; stdout hanya pesan protokol.

## Konfigurasi

Env var memakai logika yang sama, tetapi dibaca sekali oleh lapisan C++ (`native/config.cpp`,
pola `MaskConfig` xhl adopsi dari camoufox) dan divalidasi bertipe. Var yang sudah ada tetap
bekerja **tanpa perubahan**: `XHL_DATA_DIR`, `XHL_ACCOUNT`, `XHL_HTTP_PROFILE`, `XHL_NO_WAIT`,
`XHL_QUERY_IDS`, `XHL_LLM_MODEL`, `OPENAI_BASE_URL`, `OPENAI_API_KEY`.

Selain itu tersedia `XHL_CONFIG` (JSON) — berguna untuk nilai yang sebelumnya hanya bisa diubah
dengan kompilasi ulang:

```bash
XHL_CONFIG='{"retry_attempts":5,"request_timeout_secs":60}' xhl doctor
XHL_CONFIG_1='{"no_wait":true}' XHL_CONFIG_2='{"llm_model":"m"}' xhl native config
```

| Kunci | Tipe | Default | Mengatur |
|---|---|---|---|
| `http_profile` / `emulation_profile` | string | `firefox_133` | Identitas header **dan** profil TLS/HTTP2 (harus satu sumber; keduanya sinonim) |
| `no_wait` | bool | `false` | Lewati jeda antar penulisan |
| `account` | string | `default` | Akun aktif |
| `retry_attempts`, `retry_backoff_ms` | uint | `3`, `400` | Percobaan ulang kegagalan koneksi |
| `request_timeout_secs` | uint | `30` | Timeout permintaan |
| `discovery_concurrency` | uint | `8` | Unduhan bundle paralel saat discovery |
| `media_chunk_bytes` | uint | `5242880` | Ukuran potongan upload video |
| `content_max_bytes` | uint | `1048576` | Batas berkas konten (`--file`) |
| `query_ids_path` | string | – | Override daftar queryId (setara `XHL_QUERY_IDS`) |
| `profile` | string | – | Path atau JSON inline profil anti-detect |
| `draw_x`/`draw_y`/`draw_w`/`draw_h` | uint | – | Rect untuk `xhl native draw` |

`xhl native config` mencetak config yang sudah divalidasi; `xhl native selftest`
menjalankan seluruh jalur inti C++. Kunci bertipe salah **dilaporkan**, tidak diabaikan diam-diam.

## Penyamaran & koherensi identitas

xhl adalah klien HTTP, bukan browser. Yang dapat disamarkan adalah **fingerprint jaringan
(TLS/HTTP2), header HTTP, dan koherensi identitas**; nilai yang hanya muncul dari eksekusi
JavaScript (canvas hash, AudioContext, parameter WebGL runtime, `navigator` runtime) tidak
tercakup karena memerlukan mesin browser.

- **Fingerprint TLS/HTTP2** mengikuti `emulation_profile` (memakai bahan `wreq-util::Emulation`
  saat fitur `impersonate` aktif).
- **Header HTTP** dapat diganti dari profil: `headers.User-Agent` (atau `navigator.userAgent`),
  `headers.Accept-Language`, `headers.Accept-Encoding`, `headers.Accept`, `headers.DNT`,
  `headers.Sec-CH-UA*`, dan `headers.order` untuk urutan kirim. Header wajib (`cookie`,
  `x-csrf-token`, `authorization`) **tidak pernah** hilang karena profil tidak menyebutkannya.
- **Koherensi identitas** diperiksa terhadap aturan yang diport dari camoufox
  (`pythonlib/camoufox/coherence.py`) — sepuluh aturan yang menangkap kombinasi yang mustahil
  ada di mesin nyata (Apple M1 dengan 2 core, GPU Intel Mac di balik panel Apple Silicon,
  `availHeight > height`, `x86_64` bersama `armv`, dan seterusnya). Nama aturan dipertahankan
  sama persis dengan sumbernya agar dapat dibandingkan.

```bash
xhl anticheck --profile profil.json --target-os mac          # laporkan pelanggaran (exit 1 bila ada)
xhl anticheck --profile profil.json --target-os mac --apply  # perbaiki lalu cetak JSON-nya
xhl anticheck --profile profil.json --headers                # header yang akan benar-benar dikirim
XHL_CONFIG='{"profile":"profil.json"}' xhl native draw --headers   # header dari jalur produksi
```

Saat profil menyetel `headers.User-Agent` sedangkan `impersonate` aktif, **profil yang menang**:
mengirim UA yang berbeda dari fingerprint TLS justru inkonsisten.

## Impersonasi TLS (opsional)

Default memakai `reqwest` dengan Rustls. Bila X memblokir fingerprint TLS (403 pada semua endpoint
meski cookie valid), aktifkan:

```bash
cargo build --workspace --features xhl-core/impersonate
```

Fitur ini memakai [`wreq`](https://github.com/0x676e67/wreq) (BoringSSL) dan **memerlukan paket
sistem**: `cmake`, `clang`, `libclang-dev`, `nasm`. Tanpa itu build gagal dengan
`is `cmake` not installed?`. Saat fitur aktif, **user-agent tidak dikirim terpisah** karena `wreq`
menyetelnya sesuai profil TLS — mengirim keduanya justru menandai bot.

> Catatan: crate `rquest` (pendahulu `wreq`) **di-yank sepenuhnya** dari crates.io. `wreq` adalah
> penerus resmi dari repo yang sama.

## Struktur

```
crates/
├── xhl-core/   # libxhl: domain, driver, store, service, antibot, llm — tidak tahu CLI/MCP
│   └── native/ # inti C++ (config, profil, koherensi, antidetect, util) — dibangun lewat cc
├── xhl-cli/    # bin "xhl"
└── xhl-mcp/    # bin "xhl-mcp"
docs/
├── MULAI.md           # panduan mulai pakai (build → cookie → posting → MCP)
├── SYSTEM_DESIGN.md   # arsitektur, anti-bot, queryId, schema, ADR
└── WORKFLOW.md        # setup, konvensi, testing, debugging, runbook
```

Aturan dependensi: `xhl-cli`/`xhl-mcp` → `xhl-core`. Tidak pernah sebaliknya.
Inti C++ hanya memerlukan compiler dari PATH (`cc` crate) — tanpa cmake, clang, atau bindgen.

## Keamanan

- Cookie **tidak pernah** muncul di log, `Debug`, pesan error, atau output CLI/MCP; ada unit test
  yang menegakkan ini (termasuk untuk kunci API LLM).
- Berkas `xhl.db` dibatasi mode `0600`; direktori data `0700`.
- Cookie disimpan plaintext di SQLite pada iterasi ini. Abstraksi `SecretStore` disiapkan untuk
  keyring/SQLCipher tanpa refactor.
- Posting idempoten: teks+media+balasan identik pada akun yang sama tidak dikirim dua kali.

## Uji

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Uji yang memukul X nyata ditandai `#[ignore]` dan digerbangi `XHL_LIVE=1`, sehingga suite default
selalu offline dan deterministik.