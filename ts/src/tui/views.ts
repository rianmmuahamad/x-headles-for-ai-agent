/**
 * Ubah JSON dari `xhl` menjadi baris teks untuk ditampilkan di TUI.
 *
 * Semua fungsi di berkas ini murni (tanpa terminal, tanpa jaringan) sehingga
 * dapat diuji langsung. Ini pemisahan yang disengaja: bagian yang mudah salah
 * adalah pemformatan (nilai `null`, satuan waktu, kolom), bukan pemasangan
 * komponen UI.
 */

import type { Metrics, Trend, Tweet, UserProfile } from "../types.ts";
import { formatCount, relativeTime } from "../ui.ts";

/** Lebar baris tetap supaya kolom sejajar di dalam panel. */
function pad(text: string, width: number): string {
  return text.length >= width ? text : text + " ".repeat(width - text.length);
}

export function trendLines(trends: Trend[]): string[] {
  if (trends.length === 0) {
    return ["Tidak ada trend untuk kategori ini.", "X kadang tidak mengirim datanya."];
  }
  const out = ["", "  #   TOPIK                          KONTEKS"];
  out.push("  " + "─".repeat(74));
  trends.forEach((t, i) => {
    const rank = pad(String(t.rank ?? i + 1), 4);
    const name = pad(t.name.slice(0, 30), 31);
    const ctx = [t.context, t.meta_description]
      .filter((p): p is string => p !== null)
      .join(" ")
      .slice(0, 40);
    out.push(`  ${rank}${name}${ctx}`);
  });
  return out;
}

export function tweetLines(tweets: Tweet[], title?: string): string[] {
  const out: string[] = [];
  if (title) out.push("", `  ${title}`);
  if (tweets.length === 0) {
    out.push("", "  Tidak ada hasil.");
    return out;
  }
  out.push("");
  tweets.forEach((t, i) => {
    const verified = t.author.verified ? " ✓" : "";
    out.push(`  ${i + 1}. @${t.author.handle}${verified}  ·  ${relativeTime(t.created_at)}`);
    const body = t.text.replace(/\s+/g, " ").slice(0, 160);
    out.push(`     ${body}`);
    out.push(`     ${metricLine(t.metrics)}`);
    out.push(`     ${t.url}`);
    out.push("");
  });
  return out;
}

/** Engagement; `null` berarti X tidak mengirim angka itu, bukan nol. */
export function metricLine(m: Metrics | null): string {
  if (m === null) return "tanpa metrik";
  return [
    `${formatCount(m.likes)} suka`,
    `${formatCount(m.reposts)} repost`,
    `${formatCount(m.replies)} balasan`,
    `${formatCount(m.views)} lihat`,
    `${formatCount(m.bookmarks)} simpan`,
  ].join("  ·  ");
}

export function profileLines(p: UserProfile): string[] {
  const out = ["", `  ${p.display_name} ${p.verified ? "✓" : ""}`.trimEnd()];
  out.push(`  @${p.handle}`);
  out.push("");
  out.push(`  id          ${p.id}`);
  out.push(`  followers   ${formatCount(p.followers)}`);
  if (p.bio !== null && p.bio.trim() !== "") {
    out.push("");
    out.push("  bio");
    for (const line of p.bio.split("\n")) out.push(`    ${line}`);
  }
  return out;
}

export function metricsLines(id: string, m: Metrics): string[] {
  const out = ["", `  Metrics ${id}`, ""];
  out.push(`    suka       ${formatCount(m.likes)}`);
  out.push(`    repost     ${formatCount(m.reposts)}`);
  out.push(`    balasan    ${formatCount(m.replies)}`);
  out.push(`    lihat      ${formatCount(m.views)}`);
  out.push(`    simpan     ${formatCount(m.bookmarks)}`);
  return out;
}

/** Keluaran `xhl doctor` sudah berbentuk teks; hanya diberi indentasi. */
export function doctorLines(raw: string): string[] {
  const lines = raw.split("\n").filter((l) => l.trim() !== "");
  if (lines.length === 0) return ["Tidak ada keluaran dari xhl doctor."];
  return ["", ...lines.map((l) => `  ${l}`)];
}

/** Pesan status yang seragam untuk kegagalan. */
export function errorLines(message: string, hint?: string): string[] {
  const out = ["", `  GAGAL: ${message}`];
  if (hint !== undefined && hint !== "") {
    for (const line of hint.split("\n")) out.push(`    ${line}`);
  }
  return out;
}

/** Bungkus teks panjang per kata agar rapi di dalam panel. */
export function wrapLines(text: string, width: number): string[] {
  const out: string[] = [];
  for (const paragraph of text.split("\n")) {
    let line = "";
    for (const word of paragraph.split(/\s+/)) {
      if (word === "") continue;
      if (line === "") line = word;
      else if (line.length + 1 + word.length <= width) line += ` ${word}`;
      else {
        out.push(line);
        line = word;
      }
    }
    out.push(line);
  }
  return out;
}