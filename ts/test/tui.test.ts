/**
 * Uji TUI dengan renderer sungguhan dari OpenTUI (`createTestRenderer`).
 *
 * Yang diuji adalah **layar**: matriks karakter yang benar-benar dirender. Itu
 * satu-satunya cara menguji TUI secara jujur — memeriksa komponen dari luar
 * tidak membuktikan apa pun tentang apa yang dilihat pengguna.
 *
 * Perintah jaringan diuji dengan binary palsu yang mengeluarkan JSON tetap,
 * sehingga tidak ada permintaan keluar dan hasilnya deterministik.
 *
 * Catatan: OpenTUI mencetak `Error destroying root renderable` ke stderr saat
 * `createTestRenderer` dibersihkan (jalur `destroyRecursively`). Itu artefak
 * teardown renderer uji, **bukan** bug xmot: TUI sungguhan yang dijalankan di
 * PTY lalu keluar dengan `q` tidak menghasilkan pesan itu sama sekali.
 * Karena itu uji di bawah tidak memaksakan stderr bersih.
 */

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { createTestRenderer } from "@opentui/core/testing";

import { buildUi, runTask, show } from "../src/tui/app.ts";
import { metricLine, trendLines, tweetLines, errorLines, wrapLines } from "../src/tui/views.ts";
import type { Metrics, Trend, Tweet } from "../src/types.ts";

// ------------------------------------------------------- pemformatan murni ---

describe("views", () => {
  test("metricLine menandai metrik absen, bukan menampilkan null", () => {
    // Regresi: serde mengirim `null` eksplisit untuk Option::None.
    const m: Metrics = {
      likes: 1200,
      reposts: null,
      replies: 3,
      views: null,
      bookmarks: 0,
    };
    const line = metricLine(m);
    expect(line).toContain("1.2K suka");
    expect(line).toContain("- repost");
    expect(line).toContain("3 balasan");
    expect(line).not.toContain("null");
  });

  test("metricLine tanpa metrik tidak mengarang angka", () => {
    expect(metricLine(null)).toBe("tanpa metrik");
  });

  test("trendLines menyertakan peringkat dan konteks", () => {
    const trends: Trend[] = [
      { name: "#rustlang", rank: 1, context: "Teknologi", meta_description: "(", url: null },
      { name: "#lain", rank: null, context: null, meta_description: null, url: null },
    ];
    const lines = trendLines(trends);
    expect(lines.join("\n")).toContain("#rustlang");
    expect(lines.join("\n")).toContain("Teknologi");
    // Peringkat absen memakai urutan sebagai gantinya, tanpa "null" di layar.
    expect(lines[2]).not.toContain("null");
  });

  test("trendLines kosong menjelaskan, bukan layar hampa", () => {
    const lines = trendLines([]);
    expect(lines.join(" ")).toContain("Tidak ada trend");
  });

  test("tweetLines menampilkan handle, waktu, dan isi", () => {
    const tweets: Tweet[] = [
      {
        id: "1",
        author: { handle: "rustlang", display_name: "Rust", verified: true },
        text: "Rust 1.90 dirilis",
        created_at: new Date(Date.now() - 3 * 86_400_000).toISOString(),
        media: [],
        metrics: { likes: 10, reposts: 2, replies: 1, views: 500, bookmarks: null },
        url: "https://x.com/rustlang/status/1",
      },
    ];
    const out = tweetLines(tweets, "cari: rust").join("\n");
    expect(out).toContain("@rustlang");
    expect(out).toContain("3d lalu");
    expect(out).toContain("Rust 1.90 dirilis");
    expect(out).not.toContain("null");
  });

  test("errorLines menyertakan petunjuk bila ada", () => {
    const out = errorLines("session kedaluwarsa", "jalankan: xhl auth import").join("\n");
    expect(out).toContain("GAGAL: session kedaluwarsa");
    expect(out).toContain("xhl auth import");
  });

  test("wrapLines melipat pada batas kata", () => {
    const out = wrapLines("satu dua tiga empat lima", 10);
    expect(out.every((l) => l.length <= 10)).toBe(true);
    expect(out.join(" ")).toBe("satu dua tiga empat lima");
  });
});

// ------------------------------------------------------- layar sungguhan ---

/** Binary palsu: membalas JSON tetap sehingga uji tidak menyentuh jaringan. */
async function makeFakeXhl(dir: string): Promise<string> {
  const script = join(dir, "xhl-fake.sh");
  const payloads: Record<string, string> = {
    "trends ": JSON.stringify([
      { name: "#satu", rank: 1, context: "Uji", meta_description: "10 posting", url: null },
      { name: "#dua", rank: 2, context: null, meta_description: null, url: null },
    ]),
    "search ": JSON.stringify([
      {
        id: "9",
        author: { handle: "uji", display_name: "Uji", verified: false },
        text: "tweet contoh untuk uji tampilan",
        created_at: new Date().toISOString(),
        media: [],
        metrics: { likes: 5, reposts: null, replies: null, views: null, bookmarks: null },
        url: "https://x.com/uji/status/9",
      },
    ]),
    "user ": JSON.stringify({
      id: "42",
      handle: "uji",
      display_name: "Akun Uji",
      verified: true,
      bio: null,
      followers: 1500,
    }),
  };
  const body = `#!/bin/bash
args="$*"
case "$args" in
  *"doctor"*) echo "akun      : default"; echo "cookie    : belum ada"; exit 0 ;;
  *"queries list"*) echo "Daftar query ID (2 operasi):"; echo "  A  abc"; exit 0 ;;
  *"trends"*) cat <<'JSON'
${payloads["trends "]}
JSON
    exit 0 ;;
  *"search"*) cat <<'JSON'
${payloads["search "]}
JSON
    exit 0 ;;
  *"user"*) cat <<'JSON'
${payloads["user "]}
JSON
    exit 0 ;;
  *) echo "tidak ditangani: $args" >&2; exit 1 ;;
esac
`;
  await writeFile(script, body);
  await chmod(script, 0o755);
  return script;
}

describe("layar TUI (renderer sungguhan)", () => {
  let dir: string;
  let previousBin: string | undefined;

  beforeAll(async () => {
    dir = await mkdtemp(join(tmpdir(), "xmot-tui-"));
    previousBin = process.env["XMOT_XHL_BIN"];
    process.env["XMOT_XHL_BIN"] = await makeFakeXhl(dir);
  });

  afterAll(async () => {
    if (previousBin === undefined) delete process.env["XMOT_XHL_BIN"];
    else process.env["XMOT_XHL_BIN"] = previousBin;
    await rm(dir, { recursive: true, force: true });
  });

  test("layout awal tergambar: header, daftar tugas, baris argumen", async () => {
    const { renderer, renderOnce, captureCharFrame } = await createTestRenderer({
      width: 100,
      height: 30,
    });
    try {
      buildUi(renderer);
      await renderOnce();
      const frame = captureCharFrame();
      expect(frame).toContain("xmot");
      expect(frame).toContain("TUGAS");
      expect(frame).toContain("Kesehatan sesi");
      expect(frame).toContain("Cari tweet");
      expect(frame).toContain("argumen");
    } finally {
      renderer.destroy();
    }
  });

  test("hasil trend tampil di panel kanan, termasuk peringkat", async () => {
    const { renderer, renderOnce, captureCharFrame } = await createTestRenderer({
      width: 100,
      height: 30,
    });
    try {
      const ui = buildUi(renderer);
      await runTask(ui, "trends", "trending");
      await renderOnce();
      const frame = captureCharFrame();
      expect(frame).toContain("#satu");
      expect(frame).toContain("#dua");
      expect(frame).toContain("Uji");
      // Panel kiri tetap ada: hasil tidak menimpa tata letak.
      expect(frame).toContain("TUGAS");
    } finally {
      renderer.destroy();
    }
  });

  test("hasil cari menampilkan tweet dan metrik tanpa kata null", async () => {
    const { renderer, renderOnce, captureCharFrame } = await createTestRenderer({
      width: 100,
      height: 30,
    });
    try {
      const ui = buildUi(renderer);
      await runTask(ui, "search", "contoh");
      await renderOnce();
      const frame = captureCharFrame();
      expect(frame).toContain("@uji");
      expect(frame).toContain("tweet contoh");
      expect(frame).toContain("5 suka");
      expect(frame).not.toContain("null");
    } finally {
      renderer.destroy();
    }
  });

  test("argumen kosong ditolak dengan petunjuk, bukan diam", async () => {
    const { renderer, renderOnce, captureCharFrame } = await createTestRenderer({
      width: 100,
      height: 30,
    });
    try {
      const ui = buildUi(renderer);
      await runTask(ui, "user", "");
      await renderOnce();
      const frame = captureCharFrame();
      expect(frame).toContain("Handle kosong");
    } finally {
      renderer.destroy();
    }
  });

  test("kegagalan xhl tampil sebagai pesan, bukan layar kosong", async () => {
    const { renderer, renderOnce, captureCharFrame } = await createTestRenderer({
      width: 100,
      height: 30,
    });
    try {
      const ui = buildUi(renderer);
      // Tugas ini memanggil cabang yang tidak ditangani binary palsu → keluar 1.
      await runTask(ui, "metrics", "123");
      await renderOnce();
      const frame = captureCharFrame();
      expect(frame).toContain("GAGAL");
      expect(frame).toContain("gagal (kode 1)");
      // Petunjuk ikut tampil, sehingga pengguna tahu ke mana harus melihat.
      expect(frame).toContain("keluaran xhl");
    } finally {
      renderer.destroy();
    }
  });

  test("show mengganti isi panel dan menggulir ke atas", async () => {
    const { renderer, renderOnce, captureCharFrame } = await createTestRenderer({
      width: 60,
      height: 12,
    });
    try {
      const ui = buildUi(renderer);
      const long = Array.from({ length: 80 }, (_, i) => `  baris ${i}`).join("\n");
      show(ui, long.split("\n"));
      await renderOnce();
      const frame = captureCharFrame();
      // Panjang melebihi tinggi panel; yang terlihat harus bagian awal saja.
      expect(frame).toContain("baris 0");
      expect(frame).not.toContain("baris 79");
    } finally {
      renderer.destroy();
    }
  });
});