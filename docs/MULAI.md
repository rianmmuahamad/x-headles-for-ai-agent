# Mulai Pakai `xhl`

Panduan langkah demi langkah. Semua perintah dijalankan dari direktori repo.

---

## 0. Build

```bash
cargo build --workspace --release
```

Binary: `target/release/xhl` (CLI) dan `target/release/xhl-mcp` (MCP server).
Pasang ke `PATH` bila mau dipanggil dari mana saja:

```bash
install -m755 target/release/xhl target/release/xhl-mcp ~/.local/bin/
```

---

## 1. Ambil cookie sesi dari browser

`xhl` **tidak** login sendiri. Ia memakai cookie sesi yang sudah ada di browser Anda.

1. Login ke `https://x.com` di browser seperti biasa.
2. Ekspor cookie untuk domain `.x.com`. Pilih salah satu cara:

| Cara | Hasil |
|---|---|
| Extension "Get cookies.txt LOCALLY" / "Cookie-Editor" | file `cookies.txt` (format Netscape) atau JSON |
| DevTools → Application → Cookies → `https://x.com` | salin manual `auth_token` dan `ct0` |

Dua cookie yang **wajib** ada:

| Cookie | Peran |
|---|---|
| `auth_token` | bukti login |
| `ct0` | CSRF; `xhl` mengirimnya juga sebagai header `x-csrf-token` |

> Cookie ini setara akses penuh ke akun. Simpan di tempat aman, jangan di-commit
> (`.gitignore` sudah menutup `cookies.txt` dan `*.cookies.json`).

---

## 2. Impor cookie

```bash
# dari berkas (format dideteksi otomatis: Netscape / JSON / string)
./target/release/xhl auth import --from cookies.txt

# atau dari stdin
printf 'auth_token=AAAA; ct0=BBBB' | ./target/release/xhl auth import
```

Cek hasilnya:

```bash
./target/release/xhl auth status
./target/release/xhl doctor
```

`doctor` adalah pintu diagnosis. Contoh keluaran saat sehat:

```
akun      : default
database  : /home/anda/.local/share/xhl/xhl.db
cookie    : ada
profile   : firefox_133
session   : ok (@namaakun)
signer    : ready
queries   : 24 operasi diketahui
driver    : tersedia
verdict   : siap
```

Saat cookie sudah tidak berlaku:

```
session   : kedaluwarsa (HTTP 401)
verdict   : cookie tersimpan tetapi tidak berlaku — impor ulang dari browser
```

→ ulangi langkah 1–2.

---

## 3. Mulai dari yang aman (baca saja)

Urutan ini memverifikasi rantai cookie → signer → queryId tanpa mengubah apa pun di akun:

```bash
./target/release/xhl user --handle jack
./target/release/xhl search "rust async" --limit 5
./target/release/xhl timeline --kind home --limit 5
```

Tambah `--json` bila mau diproses program lain:

```bash
./target/release/xhl --json search "rust async" --limit 5
```

Bila di sini muncul `403`, jalankan `xhl doctor` dan lihat tabel triage di
`docs/WORKFLOW.md` §6 (penyebab tersering: `ct0` tidak sinkron, signer basi,
fingerprint TLS).

> ✅ **Terverifikasi live 2026-09-30** dengan sesi nyata: `doctor` melaporkan `session: ok`,
> `search`/`user`/`timeline` mengembalikan data nyata, dan `post` benar-benar menerbitkan
> tweet. Bila muncul `403` berulang di semua endpoint padahal `doctor` bilang sesi sehat,
> lihat `docs/WORKFLOW.md` §6 (fingerprint TLS → aktifkan fitur `impersonate`).

---

## 4. Posting

```bash
./target/release/xhl post "halo dari xhl"
./target/release/xhl post "dengan gambar" --media foto.png
./target/release/xhl post "balasan" --reply-to 1234567890123456789
```

Catatan:

- Panjang dihitung ala X: setiap URL = 23 karakter, bukan panjang literalnya.
- Maksimum 4 gambar **atau** 1 video per postingan; keduanya tidak bisa dicampur.
- Posting **idempoten**: teks+media+balasan identik pada akun yang sama tidak
  dikirim dua kali. Menjalankan ulang perintah yang sama tidak membuat tweet baru.
- Kedua hal di atas ditegakkan sebelum menyentuh jaringan — kegagalan validasi
  tidak memakai kuota rate limit.

Thread: CLI belum punya subcommand `thread`. Dua cara yang benar-benar jalan:

```bash
# a) rangkaian balasan manual — ID tweet sebelumnya dipakai sebagai --reply-to
./target/release/xhl --json post "1/3 Bagian pertama" 
#   -> catat "id" dari keluaran JSON
./target/release/xhl post "2/3 Bagian kedua" --reply-to <id-tadi>
./target/release/xhl post "3/3 Bagian ketiga" --reply-to <id-kedua>
```

```bash
# b) lewat MCP: satu panggilan, rantai balasan ditangani otomatis
#    tool: xhl_thread_post  args: {"parts": ["1/3 ...", "2/3 ...", "3/3 ..."]}
```

---

## 5. Analytics

Ambil metrics dan simpan snapshot lokal:

```bash
./target/release/xhl metrics 1234567890123456789
```

Karena `delta` butuh dua snapshot, jalankan lagi beberapa saat kemudian:

```bash
./target/release/xhl metrics 1234567890123456789      # snapshot kedua
./target/release/xhl metrics --delta 1234567890123456789
./target/release/xhl metrics --history 1234567890123456789
```

---

## 6. Draft (dan LLM opsional)

```bash
# draft yang Anda tulis sendiri (tidak menyentuh jaringan)
./target/release/xhl draft new --body "teks draft" --name peluncuran
./target/release/xhl draft list
./target/release/xhl draft post <draft-id>     # satu-satunya jalur draft -> tweet
```

Untuk `draft generate`, siapkan provider dulu:

```bash
export OPENAI_BASE_URL="https://api.openai.com/v1"
export OPENAI_API_KEY="sk-..."
export XHL_LLM_MODEL="gpt-4o-mini"          # opsional

./target/release/xhl draft generate --prompt "tweet singkat tentang Rust async"
```

Tanpa kedua env itu, perintah ini gagal dengan pesan yang menyebut
`OPENAI_BASE_URL + OPENAI_API_KEY` — bukan tebakan. Draft dari LLM **tidak pernah**
otomatis terkirim; Anda harus memanggil `draft post` secara eksplisit.

---

## 7. Penjadwalan

```bash
./target/release/xhl schedule post --at "2026-10-01T09:00:00+07:00" --text "terjadwal"
./target/release/xhl schedule post --at "2026-10-01T09:00:00+07:00" --draft <draft-id>
./target/release/xhl schedule list

# jalankan satu putaran (uji cepat)
./target/release/xhl run --once

# daemon; Ctrl-C untuk berhenti
./target/release/xhl run
./target/release/xhl run --interval 60

./target/release/xhl schedule cancel <job-id> --confirm
```

Job disimpan di SQLite, jadi jadwal bertahan walau proses mati. Yang gagal karena
jaringan dicoba ulang dengan backoff, yang butuh tindakan manusia (cookie mati,
anti-bot) langsung masuk `Failed` tanpa diulang sia-sia.

---

## 8. Dipakai agent lewat MCP

Daftarkan di klien MCP (omp, Claude Desktop, dsb):

```json
{
  "mcpServers": {
    "xhl": { "command": "/home/anda/.local/bin/xhl-mcp", "args": ["--account", "default"] }
  }
}
```

Uji manual via stdio:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | ./target/release/xhl-mcp
```

18 tool tersedia; daftarnya ada di `README.md`. Yang perlu diketahui agent:

- Cookie tidak pernah menjadi argumen atau hasil tool — agent hanya memanggil operasi.
- Kegagalan (validasi, sesi mati, rate limit) dikembalikan sebagai hasil tool dengan
  `isError: true` beserta pesan actionable.
- `xhl_schedule_cancel` menolak tanpa `confirm: true`.

Jalankan agent dengan `XHL_NO_WAIT=1` bila tool tidak boleh menggantung menunggu
jendela rate limit.

---

## 9. Coba tanpa akun

Seluruh CLI bisa diuji offline dengan data sintetis:

```bash
./target/release/xhl --driver fake search "rust" --limit 3
./target/release/xhl --driver fake post "uji"
./target/release/xhl --driver fake doctor
```

`--driver fake` **selalu** menghasilkan data palsu. Jangan dipakai untuk menilai
perilaku X yang sebenarnya.

---

## 10. Bila akun kena batas / diblokir

1. Hentikan `xhl run` lebih dulu.
2. `xhl doctor` — apakah cookie masih valid?
3. Data yang dikumpulkan `doctor` (`session`, `signer`, `queries`) menunjukkan
   lapisan mana yang gagal.
4. Triage lengkap: `docs/WORKFLOW.md` §6 dan runbook §8.

Bila 403 muncul di **semua** endpoint padahal cookie valid, itu indikasi
fingerprint TLS. Solusinya fitur `impersonate` (butuh `cmake clang libclang-dev nasm`):

```bash
cargo build --workspace --release --features xhl-core/impersonate
```

---

## Referensi cepat

| Perintah | Fungsi |
|---|---|
| `xhl auth import --from <file>` | impor cookie |
| `xhl auth status` | cek cookie tersimpan |
| `xhl doctor` | diagnosis lengkap |
| `xhl user --handle <h>` | profil |
| `xhl search "<q>" --limit N` | cari tweet |
| `xhl timeline --kind home\|user` | timeline |
| `xhl post "<teks>" \| --file <path> [--media f]` | posting (path dari agent) |
| `xhl trends [--category ...]` | topik ramai |
| `xhl search "<q>" --top` | cari yang paling rame |
| `xhl metrics <id> [--history\|--delta]` | analytics |
| `xhl draft ...` | kelola draft |
| `xhl schedule ...` / `xhl run` | penjadwalan |
| `xhl queries list\|refresh` | cache `queryId` |
| `xhl native config\|selftest\|profile\|draw` | inti C++: config tervalidasi, selftest, profil, gambar |
| `xhl anticheck --profile <f> [--apply] [--headers]` | koherensi identitas + header hasil derivasi |
| `xhl debug raw --op <Op> --vars '<json>'` | panggil GraphQL mentah |

Variabel lingkungan: `XHL_DATA_DIR`, `XHL_ACCOUNT`, `XHL_LOG`, `XHL_HTTP_PROFILE`,
`XHL_NO_WAIT`, `XHL_QUERY_IDS`, `OPENAI_BASE_URL`, `OPENAI_API_KEY`, `XHL_LLM_MODEL`.
Selain itu `XHL_CONFIG` (JSON) untuk nilai lanjutan (`retry_attempts`, `emulation_profile`,
`profile`, `content_max_bytes`, …) — lihat tabel di `README.md`. Env var lama tetap bekerja
tanpa perubahan.
