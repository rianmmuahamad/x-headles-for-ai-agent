/**
 * Uji perilaku xmot pada batasnya: parser argumen (murni), format angka/waktu
 * (murni), dan penemuan binary (menyentuh filesystem).
 *
 * Sisi jaringan tidak diuji di sini — itu wilayah `xhl`; xmot hanya memformat.
 */

import { describe, expect, test } from "bun:test";
import { chmod, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { parseArgs } from "../src/main.ts";
import { discoverXhl, ENV_OVERRIDE, isDiscoveryError } from "../src/xhl.ts";
import { formatCount, relativeTime, table } from "../src/ui.ts";

describe("parseArgs", () => {
  test("flag dengan nilai berpasangan", () => {
    expect(parseArgs(["--limit", "20"])).toEqual({
      positionals: [],
      flags: { limit: "20" },
    });
  });

  test("flag boolean saat tidak diikuti nilai", () => {
    expect(parseArgs(["--json", "search"])).toEqual({
      positionals: ["search"],
      flags: { json: true },
    });
  });

  test("flag boolean di akhir argv", () => {
    expect(parseArgs(["trends", "--top"])).toEqual({
      positionals: ["trends"],
      flags: { top: true },
    });
  });

  test("bentuk --nama=nilai", () => {
    expect(parseArgs(["--limit=5"])).toEqual({
      positionals: [],
      flags: { limit: "5" },
    });
  });

  test("flag berulang digabung, sehingga media banyak tetap utuh", () => {
    const { flags } = parseArgs(["--media", "a.png", "--media", "b.jpg"]);
    expect(flags["media"]).toBe("a.png\0b.jpg");
  });

  test("nilai yang diawali tanda hubung dianggap nilai, bukan flag", () => {
    // `-` (stdin) harus sampai ke perintah, bukan ditelan parser.
    expect(parseArgs(["--file", "-"]).flags["file"]).toBe("-");
  });

  test("posisi dan flag dapat bercampur", () => {
    const { positionals, flags } = parseArgs([
      "search",
      "kata kunci",
      "--top",
      "--limit",
      "30",
    ]);
    expect(positionals).toEqual(["search", "kata kunci"]);
    expect(flags["top"]).toBe(true);
    expect(flags["limit"]).toBe("30");
  });

  test("flag boolean tidak menelan kata posisi berikutnya", () => {
    // Regresi: sebelumnya `--json search` menyimpan json="search" sehingga
    // perintah `search` ikut hilang dari argv.
    expect(parseArgs(["--json", "search", "kata kunci"])).toEqual({
      positionals: ["search", "kata kunci"],
      flags: { json: true },
    });
    expect(parseArgs(["search", "--top", "rust"])).toEqual({
      positionals: ["search", "rust"],
      flags: { top: true },
    });
  });
});

describe("discoverXhl", () => {
  test("override yang menunjuk berkas tidak ada ditolak dengan saran", () => {
    const before = process.env[ENV_OVERRIDE];
    process.env[ENV_OVERRIDE] = "/tmp/tidak-ada-xmot-xyz";
    try {
      const found = discoverXhl("/tmp");
      expect(isDiscoveryError(found)).toBe(true);
      if (isDiscoveryError(found)) {
        expect(found.hint).toContain(ENV_OVERRIDE);
      }
    } finally {
      if (before === undefined) delete process.env[ENV_OVERRIDE];
      else process.env[ENV_OVERRIDE] = before;
    }
  });

  test("override berupa nama polos diserahkan ke PATH", () => {
    const before = process.env[ENV_OVERRIDE];
    process.env[ENV_OVERRIDE] = "xhl";
    try {
      const found = discoverXhl("/tmp");
      // Boleh ditemukan di PATH maupun gagal bila tidak terpasang; yang penting
      // tidak salah dibaca sebagai jalur relatif.
      if (!isDiscoveryError(found)) {
        expect(found.origin).toBe("env");
        expect(found.command).toBe("xhl");
      }
    } finally {
      if (before === undefined) delete process.env[ENV_OVERRIDE];
      else process.env[ENV_OVERRIDE] = before;
    }
  });

  test("ditemukan di repo lewat penelusuran ke atas, meski cwd di luar", async () => {
    const before = process.env[ENV_OVERRIDE];
    delete process.env[ENV_OVERRIDE];
    const fakeRepo = await mkdtemp(join(tmpdir(), "xmot-repo-"));
    const releaseDir = join(fakeRepo, "target", "release");
    await mkdir(releaseDir, { recursive: true });
    const fakeBin = join(releaseDir, "xhl");
    await writeFile(fakeBin, "#!/bin/sh\nexit 0\n");
    await chmod(fakeBin, 0o755);
    const nested = join(fakeRepo, "ts", "src");
    await mkdir(nested, { recursive: true });

    try {
      const found = discoverXhl(nested);
      if (!isDiscoveryError(found)) {
        expect(found.origin).toBe("repo");
        expect(found.command).toBe(fakeBin);
      }
    } finally {
      if (before !== undefined) process.env[ENV_OVERRIDE] = before;
      await rm(fakeRepo, { recursive: true, force: true });
    }
  });
});

describe("formatCount", () => {
  test("nilai kecil apa adanya", () => {
    expect(formatCount(0)).toBe("0");
    expect(formatCount(999)).toBe("999");
  });

  test("ribuan dan jutaan disingkat", () => {
    expect(formatCount(1200)).toBe("1.2K");
    expect(formatCount(12_345)).toBe("12K");
    expect(formatCount(3_500_000)).toBe("3.5M");
  });

  test("absen ditandai, bukan dikarang jadi 0", () => {
    expect(formatCount(undefined)).toBe("-");
    // Regresi: serde mengirim `null`, dan sebelumnya tampil sebagai "null".
    expect(formatCount(null)).toBe("-");
  });
});

describe("relativeTime", () => {
  test("waktu lampau dalam detik dan hari", () => {
    const now = Date.now();
    expect(relativeTime(new Date(now - 30_000).toISOString())).toBe("30s lalu");
    // Regresi: pembagi 86.400 pernah salah dilabeli "h" sehingga 3 hari tampil
    // sebagai "3h lalu".
    expect(relativeTime(new Date(now - 3 * 86_400_000).toISOString())).toBe("3d lalu");
    expect(relativeTime(new Date(now - 2 * 3_600_000).toISOString())).toBe("2h lalu");
    expect(relativeTime(new Date(now - 400 * 86_400_000).toISOString())).toBe("1y lalu");
  });

  test("timestamp tidak sah dikembalikan apa adanya", () => {
    expect(relativeTime("bukan-waktu")).toBe("bukan-waktu");
  });
});

describe("table", () => {
  test("lebar kolom mengikuti isi terpanjang", () => {
    const out = table(["a", "b"], [["x", "yy"], ["zzz", "w"]]);
    const lines = out.split("\n");
    expect(lines).toHaveLength(3);
    expect(lines[0]).toContain("a");
    // Kolom kedua rata pada lebar "yy".
    expect(lines[2]).toContain("w");
  });

  test("teks panjang dilipat, tidak dipotong", () => {
    const long = "kata ".repeat(30).trim();
    const out = table(["pesan"], [[long]], { maxWidth: 30 });
    const joined = out.replace(/\s+/g, " ");
    expect(joined).toContain("kata kata kata");
    // Tidak ada kata yang hilang: lipatan mempertahankan seluruh isi.
    expect(joined.match(/kata/g)?.length).toBe(30);
  });
});
