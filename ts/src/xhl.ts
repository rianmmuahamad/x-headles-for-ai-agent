/**
 * Penemuan binary `xhl`.
 *
 * `xmot` adalah antarmuka tipis di atas `xhl`: ia tidak mengimplementasikan
 * protokol X sendiri. Karena itu aturan pertama adalah menemukan binary yang
 * benar, dan bila tidak ketemu **berhenti dengan pesan yang menuntun** — bukan
 * mencoba menebak atau menirukan perilaku xhl.
 */

import { existsSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Env var untuk menunjuk binary secara eksplisit (jalur atau nama di PATH). */
export const ENV_OVERRIDE = "XMOT_XHL_BIN";

/** Nama yang dicoba di PATH bila tidak ada override. */
const PATH_NAMES = ["xhl", "xhl.exe"];

/** Jalur rilis relatif terhadap akar repo, dipakai saat dijalankan dari sumber. */
const REPO_RELEASE = ["target", "release", "xhl"] as const;

export type Discovery = {
  /** Perintah yang akan dijalankan. */
  command: string;
  /** Dari mana ia ditemukan — dipakai `xmot doctor`. */
  origin: "env" | "path" | "repo";
};

export type DiscoveryError = {
  message: string;
  /** Saran konkret untuk pengguna; wajib ada agar kesalahan dapat ditindak. */
  hint: string;
};

/** Cari `xhl` di PATH tanpa menjalankan shell. */
function findInPath(): string | null {
  const pathValue = process.env["PATH"] ?? "";
  const sep = process.platform === "win32" ? ";" : ":";
  for (const dir of pathValue.split(sep)) {
    if (dir === "") continue;
    for (const name of PATH_NAMES) {
      const candidate = join(dir, name);
      if (isExecutable(candidate)) return candidate;
    }
  }
  return null;
}

function isExecutable(p: string): boolean {
  try {
    const st = statSync(p);
    if (!st.isFile()) return false;
    // Windows tidak mengenal bit eksekusi; keberadaan berkas sudah cukup.
    if (process.platform === "win32") return true;
    return (st.mode & 0o111) !== 0;
  } catch {
    return false;
  }
}

/**
 * Telusuri ke atas dari `startDir` mencari `target/release/xhl`.
 *
 * Berguna saat `xmot` dipakai langsung dari repo tanpa memasang `xhl` ke PATH —
 * kasus paling umum sebelum proyek dipasang.
 */
function findInRepo(startDir: string): string | null {
  let dir = resolve(startDir);
  for (let depth = 0; depth < 10; depth++) {
    const candidate = join(dir, ...REPO_RELEASE);
    if (isExecutable(candidate)) return candidate;
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return null;
}

/**
 * Temukan binary `xhl`.
 *
 * Urutan: override eksplisit → PATH → `target/release/xhl` di repo.
 * Override diperlakukan sebagai perintah apa adanya (boleh jalur absolut),
 * karena pengguna yang menyetelnya tahu apa yang ia mau.
 */
export function discoverXhl(cwd: string = process.cwd()): Discovery | DiscoveryError {
  const override = process.env[ENV_OVERRIDE]?.trim();
  if (override && override !== "") {
    // Jalur relatif diartikan relatif terhadap cwd; nama polos diserahkan ke PATH.
    const looksLikePath = override.includes("/") || override.includes("\\");
    if (looksLikePath && !existsSync(resolve(cwd, override))) {
      return {
        message: `${ENV_OVERRIDE} menunjuk berkas yang tidak ada: ${override}`,
        hint: `Betulkan ${ENV_OVERRIDE}, atau hapus agar xmot mencari xhl sendiri.`,
      };
    }
    return { command: looksLikePath ? resolve(cwd, override) : override, origin: "env" };
  }

  const inPath = findInPath();
  if (inPath) return { command: inPath, origin: "path" };

  // Dicari dari dua arah: cwd pengguna, lalu lokasi xmot itu sendiri. Tanpa
  // yang kedua, menjalankan xmot dari direktori lain (mis. dari /tmp) gagal
  // padahal binary ada di repo tempat xmot hidup.
  const here = dirname(fileURLToPath(import.meta.url));
  for (const start of [cwd, here]) {
    const inRepo = findInRepo(start);
    if (inRepo) return { command: inRepo, origin: "repo" };
  }

  return {
    message: "binary `xhl` tidak ditemukan",
    hint: [
      "xmot membutuhkan binary xhl (ia bukan implementasi ulang protokol X).",
      "  build : cargo build --release      (di akar repo xhl)",
      `  atau pasang ke PATH : export PATH="/path/ke/xhl/target/release:$PATH"`,
      `  atau tunjuk langsung : export ${ENV_OVERRIDE}=/path/ke/xhl`,
    ].join("\n"),
  };
}

/** Pembeda tipe untuk hasil `discoverXhl`. */
export function isDiscoveryError(v: Discovery | DiscoveryError): v is DiscoveryError {
  return "message" in v;
}