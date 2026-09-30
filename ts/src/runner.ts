/**
 * Menjalankan `xhl` sebagai proses anak.
 *
 * Pembagian aliran sengaja: stdout milik data, stderr diteruskan apa adanya ke
 * terminal. `xhl` menulis log `tracing` (peringatan retry, rate limit) ke
 * stderr, sehingga terlihat **selama** perintah berjalan — bukan baru muncul
 * setelah selesai bila stderr ditampung.
 */

import {
  ENV_OVERRIDE,
  discoverXhl,
  isDiscoveryError,
  type Discovery,
} from "./xhl.ts";

export type SpawnResult = {
  code: number;
  stdout: string;
};

export class XhlError extends Error {
  constructor(
    message: string,
    readonly hint?: string,
  ) {
    super(message);
    this.name = "XhlError";
  }
}

type SpawnOpts = { stdin?: string; stdout?: "pipe" | "inherit" };

/**
 * Permukaan subprocess yang dipakai modul ini.
 *
 * Ditulis konkret (bukan `ReturnType<typeof Bun.spawn>`) supaya kontraknya
 * terbaca langsung: stdin boleh pipe (kita kirim isinya) atau inherit (xhl
 * membaca stdin terminal, mis. `echo teks | xmot post --file -`); stderr selalu
 * diteruskan ke terminal.
 */
type Child = Bun.Subprocess<"pipe" | "inherit", "pipe" | "inherit", "inherit">;

let cached: Discovery | null = null;

/**
 * Argumen yang selalu disisipkan sebelum argv perintah.
 *
 * Dipakai untuk flag **global** milik xhl (`--account`, `--driver`), yang
 * kedudukannya sama seperti pada CLI xhl: satu nilai berlaku untuk seluruh
 * pemanggilan. Tanpa ini, `xmot --driver fake search ...` akan diam-diam
 * mengabaikan `--driver` dan tetap menyentuh jaringan.
 */
let globalArgs: string[] = [];

export function setGlobalArgs(args: string[]): void {
  globalArgs = args;
}

function resolveBinary(): Discovery {
  if (cached) return cached;
  const found = discoverXhl();
  if (isDiscoveryError(found)) throw new XhlError(found.message, found.hint);
  cached = found;
  return found;
}

/** Binary yang sedang dipakai; dipakai `xmot doctor`. */
export function xhlCommand(): Discovery {
  return resolveBinary();
}

/**
 * Jalankan `xhl` tanpa menafsirkan kode keluarnya.
 *
 * `xhl doctor` dan `xhl anticheck` memakai kode bukan-nol sebagai **hasil**
 * ("ada komponen gagal", "identitas tidak koheren"), bukan kegagalan program.
 * Karena itu penafsiran kode diserahkan ke pemanggil.
 */
export async function spawnXhl(
  argv: string[],
  opts: SpawnOpts = {},
): Promise<SpawnResult> {
  const { command } = resolveBinary();
  let proc: Child;
  try {
    proc = Bun.spawn([command, ...globalArgs, ...argv], {
      stdout: opts.stdout ?? "pipe",
      stderr: "inherit",
      stdin: opts.stdin === undefined ? "inherit" : "pipe",
    }) as Child;
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    throw new XhlError(
      `gagal menjalankan ${command}: ${detail}`,
      [
        "Binary mungkin tidak ada atau bukan berkas yang dapat dieksekusi.",
        `  periksa : ls -l ${command}`,
        `  atau tunjuk binary lain : export ${ENV_OVERRIDE}=/path/ke/xhl`,
      ].join("\n"),
    );
  }

  if (opts.stdin !== undefined && proc.stdin) {
    proc.stdin.write(opts.stdin);
    await proc.stdin.end();
  }

  const stdout =
    proc.stdout === undefined ? "" : await new Response(proc.stdout).text();
  const code = await proc.exited;
  return { code, stdout };
}

/** Jalankan dan anggap kode bukan-nol sebagai kegagalan. */
export async function xhlOrThrow(
  argv: string[],
  opts: SpawnOpts = {},
): Promise<SpawnResult> {
  const result = await spawnXhl(argv, opts);
  if (result.code !== 0) {
    // Detail sebab sudah tercetak di stderr oleh xhl sendiri; mengulangnya di
    // sini hanya menggandakan pesan.
    throw new XhlError(
      `xhl ${argv.join(" ")} gagal (kode ${result.code})`,
      "Sebab lengkapnya ada di keluaran xhl di atas.",
    );
  }
  return result;
}

/**
 * Jalankan `xhl --json` dan parse hasilnya.
 *
 * Keluaran divalidasi benar-benar JSON: bila ada teks lain yang tercampur,
 * menyerahkan nilai rusak ke pemanggil akan memunculkan kesalahan yang
 * membingungkan jauh dari sebabnya.
 */
export async function xhlJson<T>(argv: string[]): Promise<T> {
  const { stdout } = await xhlOrThrow([...argv, "--json"]);
  try {
    return JSON.parse(stdout) as T;
  } catch {
    const preview = stdout.slice(0, 200).replace(/\s+/g, " ").trim();
    throw new XhlError(
      `xhl ${argv.join(" ")} --json tidak mengembalikan JSON`,
      `Keluaran: ${preview || "(kosong)"}`,
    );
  }
}

/** Teruskan stdout ke terminal dan kembalikan kode keluar apa adanya. */
export async function forwardXhl(
  argv: string[],
  opts: SpawnOpts = {},
): Promise<number> {
  const { code, stdout } = await spawnXhl(argv, opts);
  if (stdout !== "") process.stdout.write(stdout);
  return code;
}