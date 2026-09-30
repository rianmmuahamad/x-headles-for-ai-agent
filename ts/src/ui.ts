/**
 * Lapisan tampilan: warna, tabel, garis, dan daftar kunci-nilai.
 *
 * Tanpa dependensi. Warna dimatikan otomatis saat keluaran bukan terminal atau
 * saat `NO_COLOR` diset, supaya keluaran `xmot` aman dipipe ke berkas/log.
 */

const useColor =
  process.env["NO_COLOR"] === undefined && process.stdout.isTTY === true;

type Style = "dim" | "bold" | "red" | "green" | "yellow" | "cyan" | "magenta";

const CODES: Record<Style, string> = {
  dim: "2",
  bold: "1",
  red: "31",
  green: "32",
  yellow: "33",
  cyan: "36",
  magenta: "35",
};

/** Bungkus teks dengan gaya ANSI, atau kembalikan apa adanya bila warna mati. */
export function style(text: string, ...styles: Style[]): string {
  if (!useColor || styles.length === 0) return text;
  const codes = styles.map((s) => CODES[s]).join(";");
  return `\u001b[${codes}m${text}\u001b[0m`;
}

/** Apakah warna aktif (dipakai perintah untuk menyesuaikan keluaran). */
export const colorEnabled = useColor;

/** Jumlah tampilan karakter: ANSI escape tidak dihitung lebar kolom. */
function displayWidth(s: string): number {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\u001b\[[0-9;]*m/g, "").length;
}

function padRight(s: string, width: number): string {
  const gap = width - displayWidth(s);
  return gap > 0 ? s + " ".repeat(gap) : s;
}

/** Judul bagian dengan garis pemisah yang jelas di terminal maupun log. */
export function heading(text: string): string {
  return `\n${style(text, "bold", "cyan")}\n${style("─".repeat(Math.max(text.length, 8)), "dim")}`;
}

/**
 * Render tabel dengan lebar kolom mengikuti isi.
 *
 * Teks yang lebih panjang dari `maxWidth` dilipat pada batas kata, bukan
 * dipotong — memotong teks tweet akan menyembunyikan isi yang justru dicari.
 */
export function table(
  headers: string[],
  rows: string[][],
  opts: { maxWidth?: number; gap?: number } = {},
): string {
  const gap = opts.gap ?? 2;
  const columns = headers.length;
  const widths = headers.map((h, i) =>
    Math.max(
      displayWidth(h),
      ...rows.map((r) => displayWidth(r[i] ?? "")),
    ),
  );

  // Lipat kolom terlebar bila total melebihi lebar terminal.
  const termWidth = process.stdout.columns ?? 100;
  const total = widths.reduce((a, b) => a + b, 0) + gap * (columns - 1);
  const overflow = total - termWidth;
  if (overflow > 0 && opts.maxWidth !== undefined) {
    const widest = widths.indexOf(Math.max(...widths));
    if (widest >= 0 && widths[widest] !== undefined) {
      widths[widest] = Math.max(12, widths[widest] - overflow);
    }
  }

  const header = headers.map((h, i) => style(padRight(h, widths[i] ?? 0), "bold"));
  const lines = [header.join(" ".repeat(gap))];

  for (const row of rows) {
    const cells = row.map((cell, i) => wrap(cell ?? "", widths[i] ?? 0));
    const height = Math.max(...cells.map((c) => c.length));
    for (let line = 0; line < height; line++) {
      lines.push(
        cells
          .map((c, i) => padRight(c[line] ?? "", widths[i] ?? 0))
          .join(" ".repeat(gap))
          .trimEnd(),
      );
    }
  }
  return lines.join("\n");
}

/** Lipat teks pada batas kata; kata yang lebih panjang dari lebar ikut dipotong. */
function wrap(text: string, width: number): string[] {
  if (width <= 0) return [text];
  const out: string[] = [];
  let line = "";
  for (const word of text.split(/\s+/)) {
    if (word === "") continue;
    if (line === "") {
      line = word;
    } else if (displayWidth(line) + 1 + displayWidth(word) <= width) {
      line += ` ${word}`;
    } else {
      out.push(line);
      line = word;
    }
    while (displayWidth(line) > width) {
      // Kata tunggal yang lebih panjang dari kolom: potong keras.
      out.push(line.slice(0, width));
      line = line.slice(width);
    }
  }
  if (line !== "") out.push(line);
  return out.length > 0 ? out : [""];
}

/** Daftar kunci-nilai dengan kunci rata kiri. */
export function keyValues(pairs: [string, string | undefined][]): string {
  const present = pairs.filter((p): p is [string, string] => p[1] !== undefined);
  if (present.length === 0) return "";
  const width = Math.max(...present.map(([k]) => displayWidth(k)));
  return present
    .map(([k, v]) => `${style(padRight(k, width), "dim")}  ${v}`)
    .join("\n");
}

/** Format angka besar ala X: 1200 -> 1.2K, 3500000 -> 3.5M. */
export function formatCount(n: number | null | undefined): string {
  // `null` dan `undefined` sama-sama berarti "tidak ada nilai"; menampilkan
  // teks "null" justru menyamar sebagai data.
  if (n === null || n === undefined) return "-";
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}K`;
  return `${(n / 1_000_000).toFixed(n < 10_000_000 ? 1 : 0)}M`;
}

/** Waktu relatif ringkas dari timestamp RFC3339. */
export function relativeTime(iso: string): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return iso;
  const seconds = Math.round((Date.now() - then) / 1000);
  if (seconds < 60) return `${seconds}s lalu`;
  // Pasangan pembagi/label harus berpasangan benar: 86.400 detik adalah satu
  // hari, bukan satu jam — kesalahan label di sini menyesatkan pembaca.
  // Urutan menurun supaya satuan terbesar yang muat dipakai lebih dulu.
  const units: [number, string][] = [
    [31_536_000, "y"],
    [604_800, "w"],
    [86_400, "d"],
    [3_600, "h"],
    [60, "m"],
  ];
  for (const [size, label] of units) {
    if (seconds >= size) return `${Math.floor(seconds / size)}${label} lalu`;
  }
  return `${seconds}s lalu`;
}

/** Cetak nilai sebagai JSON rapi (untuk `--json`). */
export function printJson(value: unknown): void {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}