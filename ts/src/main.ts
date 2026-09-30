#!/usr/bin/env bun
/**
 * `xmot` — CLI modern untuk X Headless.
 *
 * Antarmuka tipis di atas binary `xhl`: xmot memformat keluaran dan mengelola
 * alur tulis (pratinjau + konfirmasi), sedangkan seluruh protokol X, sesi, dan
 * anti-bot tetap dikerjakan `xhl`. Pembagian itu disengaja — satu implementasi
 * protokol saja, sehingga perbaikan di xhl langsung berlaku di sini.
 *
 * Parser argumen ditulis sendiri, bukan memakai framework: kebutuhan berkas ini
 * hanya ~15 perintah dengan flag `--nama nilai` dan `--nama=value`, dan riwayat
 * satu dependensi kecil jauh lebih murah daripada satu dependensi framework.
 */

import { cmdDraft, cmdPost, cmdSchedule, cmdScheduleList, cmdThreadPost } from "./commands/write.ts";
import {
  cmdDoctor,
  cmdMetrics,
  cmdQueries,
  cmdSearch,
  cmdSessionStatus,
  cmdThread,
  cmdTimeline,
  cmdTrends,
  cmdUser,
  type Flags,
} from "./commands/read.ts";
import { XhlError, setGlobalArgs } from "./runner.ts";
import { style } from "./ui.ts";

const VERSION = "0.1.0";

const USAGE = `xmot ${VERSION} — CLI untuk X Headless (xhl)

Pemakaian:
  xmot <perintah> [argumen] [--flag nilai]

Baca (tidak mengubah apa pun di X):
  trends   [--category trending|news|sport|entertainment] [--limit N]
  search   "<kata kunci>" [--top] [--limit N]
  timeline [--kind home|home_latest|user] [--handle nama] [--limit N]
  user     --handle nama
  thread   <id tweet>
  metrics  <id tweet> [--history] [--limit N]
  doctor                     kesehatan sesi, cookie, signer, queryId
  status                     ringkas status sesi tersimpan
  queries  [--refresh]

Tulis (selalu minta konfirmasi lebih dulu):
  post     --text "..." | --file <path> [--media <path>]... [--reply-to <id>] [--yes]
  utas     --text "..." | --file <path> [--yes]        (bagian dipisah baris kosong)
  schedule post --at <RFC3339> --text "..." | --file <path> [--yes]
  schedule list
  draft    list | new --text "..." | post <id draft>

Umum:
  --json          keluarkan JSON mentah (dapat dipipe ke jq)
  --yes           lewati konfirmasi (untuk skrip)
  --help          tampilkan bantuan ini
  --version       tampilkan versi

Lingkungan:
  XMOT_XHL_BIN    jalur/nama binary xhl bila tidak ada di PATH
  NO_COLOR        matikan warna
  XHL_DATA_DIR    diteruskan ke xhl: lokasi store & cookie
`;

/**
 * Flag yang menerima nilai. Selain yang terdaftar di sini dianggap boolean,
 * sehingga sebuah kata posisi tidak pernah tertelan sebagai nilai flag:
 * `xmot --json search "x"` harus tetap membaca `search` sebagai perintah.
 *
 * Nilai yang diawali `--` tidak didukung bentuk berpisah; pakai `--text=--halo`.
 */
const VALUE_FLAGS: Record<string, true> = {
  limit: true,
  category: true,
  kind: true,
  handle: true,
  user: true,
  file: true,
  text: true,
  media: true,
  "reply-to": true,
  at: true,
  account: true,
  driver: true,
};

/**
 * Pisahkan argv menjadi posisi dan flag.
 *
 * Flag bernilai yang muncul berulang digabung dengan pemisah NUL supaya
 * perintah penerima banyak nilai (`--media a --media b`) dapat mengumpulkannya
 * tanpa struktur data tambahan.
 */
export function parseArgs(argv: string[]): { positionals: string[]; flags: Flags } {
  const positionals: string[] = [];
  const flags: Flags = {};
  for (let i = 0; i < argv.length; i++) {
    const token = argv[i]!;
    if (!token.startsWith("--")) {
      positionals.push(token);
      continue;
    }
    const body = token.slice(2);
    const eq = body.indexOf("=");
    if (eq !== -1) {
      mergeFlag(flags, body.slice(0, eq), body.slice(eq + 1));
      continue;
    }
    const next = argv[i + 1];
    const wantsValue = VALUE_FLAGS[body] === true;
    if (wantsValue && next !== undefined && next !== "") {
      mergeFlag(flags, body, next);
      i++;
    } else {
      mergeFlag(flags, body, true);
    }
  }
  return { positionals, flags };
}

function mergeFlag(flags: Flags, name: string, value: string | boolean): void {
  const existing = flags[name];
  if (typeof existing === "string" && typeof value === "string") {
    flags[name] = `${existing}\0${value}`;
    return;
  }
  flags[name] = value;
}

async function dispatch(
  command: string,
  positionals: string[],
  flags: Flags,
  json: boolean,
): Promise<number> {
  const rest = positionals.slice(1);
  switch (command) {
    case "trends":
      return cmdTrends(flags, json);
    case "search":
      return cmdSearch({ ...flags, query: positionals[1] ?? "" }, json);
    case "timeline":
      return cmdTimeline(flags, json);
    case "user":
      return cmdUser(flags, json);
    case "thread":
      return cmdThread(rest, json);
    case "metrics":
      return cmdMetrics(rest, flags, json);
    case "doctor":
      return cmdDoctor(json);
    case "status":
      return cmdSessionStatus(json);
    case "queries":
      return cmdQueries(flags, json);
    case "post":
      return cmdPost(flags, json);
    case "utas":
    case "thread-post":
      return cmdThreadPost(flags, json);
    case "schedule":
      return rest[0] === "list" ? cmdScheduleList(json) : cmdSchedule(flags, json);
    case "draft":
      return cmdDraft(rest, flags, json);
    default:
      throw new Error(`perintah tidak dikenal: ${command}\n\n${USAGE}`);
  }
}

async function main(): Promise<number> {
  const argv = process.argv.slice(2);
  if (argv.includes("--help") || argv.includes("-h") || argv.length === 0) {
    process.stdout.write(USAGE);
    return argv.length === 0 ? 1 : 0;
  }
  if (argv.includes("--version") || argv.includes("-V")) {
    process.stdout.write(`xmot ${VERSION}\n`);
    return 0;
  }

  const { positionals, flags } = parseArgs(argv);
  const command = positionals[0];
  if (command === undefined) {
    process.stdout.write(USAGE);
    return 1;
  }

  // Flag global xhl diteruskan apa adanya; nilainya tunggal, bukan berulang.
  const global: string[] = [];
  for (const name of ["driver", "account"] as const) {
    const value = flags[name];
    if (typeof value === "string" && value !== "") {
      global.push(`--${name}`, value);
    }
  }
  setGlobalArgs(global);

  return dispatch(command, positionals, flags, flags["json"] === true);
}

try {
  process.exitCode = await main();
} catch (err) {
  // Pesan sebab datang dari xhl/sistem apa adanya; tidak dibungkus lapisan lain
  // supaya barisnya tetap dapat dicari di kode xhl.
  const message = err instanceof Error ? err.message : String(err);
  process.stderr.write(`${style("error:", "red")} ${message}\n`);
  if (err instanceof XhlError && err.hint) {
    process.stderr.write(`${style(err.hint, "dim")}\n`);
  }
  process.exitCode = 1;
}