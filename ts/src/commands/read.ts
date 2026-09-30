/**
 * Perintah baca: trends, search, timeline, user, thread, metrics, doctor,
 * status, queries.
 *
 * Semuanya hanya membaca. Tidak ada perintah di berkas ini yang mengubah state
 * di X — perintah tulis ada di `commands/write.ts` dan meminta konfirmasi.
 */

import { xhlJson, forwardXhl, spawnXhl, xhlOrThrow } from "../runner.ts";
import type { TimelinePage, Trend, Tweet, UserProfile } from "../types.ts";
import {
  formatCount,
  heading,
  keyValues,
  printJson,
  relativeTime,
  style,
  table,
} from "../ui.ts";

export type Flags = Record<string, string | boolean | undefined>;

/** Ambil flag sebagai string; `--limit 20` maupun `--limit=20` diterima. */
export function flagStr(flags: Flags, name: string): string | undefined {
  const v = flags[name];
  return typeof v === "string" ? v : undefined;
}

/** Flag boolean; kehadiran tanpa nilai dianggap true. */
export function flagBool(flags: Flags, name: string): boolean {
  return flags[name] === true || flags[name] === "true";
}

function limitOf(flags: Flags, fallback: number): number {
  const raw = flagStr(flags, "limit");
  if (raw === undefined) return fallback;
  const n = Number.parseInt(raw, 10);
  if (!Number.isFinite(n) || n <= 0) {
    throw new Error(`--limit harus bilangan positif, dapat: ${raw}`);
  }
  return n;
}

/** Satu baris ringkas untuk daftar tweet. */
export function tweetLine(t: Tweet): string {
  const m = t.metrics;
  const engagement = m
    ? `${formatCount(m.likes)} suka · ${formatCount(m.reposts)} repost · ${formatCount(m.views)} lihat`
    : "tanpa metrik";
  return [
    style(`@${t.author.handle}`, "cyan") + (t.author.verified ? style(" ✓", "green") : ""),
    style(relativeTime(t.created_at), "dim"),
    engagement,
  ].join("  ");
}

/** Isi tweet dilipat agar tidak menelan seluruh layar di daftar panjang. */
function tweetBody(text: string, indent = "  "): string {
  return text
    .replace(/\n+/g, " ")
    .slice(0, 280)
    .replace(/^/gm, indent);
}

export function renderTweets(tweets: Tweet[], json: boolean): void {
  if (json) {
    printJson(tweets);
    return;
  }
  if (tweets.length === 0) {
    process.stdout.write("tidak ada hasil\n");
    return;
  }
  const blocks = tweets.map((t, i) => {
    const n = style(`${i + 1}.`, "dim");
    return `${n} ${tweetLine(t)}\n${tweetBody(t.text)}\n  ${style(t.url, "dim")}`;
  });
  process.stdout.write(`${blocks.join("\n\n")}\n`);
}

// ------------------------------------------------------------------ perintah ---

export async function cmdTrends(flags: Flags, json: boolean): Promise<number> {
  const category = flagStr(flags, "category") ?? "trending";
  const limit = limitOf(flags, 20);
  const trends = await xhlJson<Trend[]>([
    "trends",
    "--category",
    category,
    "--limit",
    String(limit),
  ]);

  if (json) {
    printJson(trends);
    return 0;
  }
  if (trends.length === 0) {
    // Bukan error: X kadang tidak mengirim trend untuk kategori ini. Menampilkan
    // daftar kosong lebih jujur daripada mengarang isi.
    process.stdout.write(
      `tidak ada trend untuk kategori '${category}' (X mungkin tidak mengirim datanya)\n`,
    );
    return 0;
  }
  process.stdout.write(heading(`Trend: ${category}`) + "\n");
  const rows = trends.map((t, i) => [
    String(t.rank ?? i + 1),
    t.name,
    [t.context, t.meta_description].filter((part): part is string => part !== null).join(" "),
  ]);
  process.stdout.write(`${table(["#", "Topik", "Konteks"], rows, { maxWidth: 70 })}\n`);
  return 0;
}

export async function cmdSearch(flags: Flags, json: boolean): Promise<number> {
  const query = flagStr(flags, "query");
  if (query === undefined || query.trim() === "") {
    throw new Error("berikan kata kunci: xmot search \"kata kunci\"");
  }
  const limit = limitOf(flags, 20);
  const ranked = flagBool(flags, "top") || flagBool(flags, "ranked");
  const argv = ["search", query, "--limit", String(limit)];
  if (ranked) argv.push("--top");

  const tweets = await xhlJson<Tweet[]>(argv);
  if (!json) {
    process.stdout.write(
      heading(`Cari: ${query}${ranked ? " (paling rame)" : " (terbaru)"}`) + "\n",
    );
  }
  renderTweets(tweets, json);
  return 0;
}

export async function cmdTimeline(flags: Flags, json: boolean): Promise<number> {
  const kind = flagStr(flags, "kind") ?? "home";
  const handle = flagStr(flags, "handle");
  if (kind === "user" && (handle === undefined || handle === "")) {
    throw new Error("--kind user membutuhkan --handle <nama>");
  }
  const limit = limitOf(flags, 20);
  const argv = ["timeline", "--kind", kind, "--limit", String(limit)];
  if (handle !== undefined && handle !== "") argv.push("--handle", handle);

  const page = await xhlJson<TimelinePage>(argv);
  if (!json) {
    process.stdout.write(
      heading(`Timeline: ${kind}${handle ? ` @${handle}` : ""}`) + "\n",
    );
  }
  renderTweets(page.items, json);
  return 0;
}

export async function cmdUser(flags: Flags, json: boolean): Promise<number> {
  const handle = flagStr(flags, "handle") ?? flagStr(flags, "user");
  if (handle === undefined || handle === "") {
    throw new Error("berikan handle: xmot user --handle nama");
  }
  const profile = await xhlJson<UserProfile>(["user", "--handle", handle]);
  if (json) {
    printJson(profile);
    return 0;
  }
  const title =
    profile.display_name + (profile.verified ? style(" ✓", "green") : "");
  process.stdout.write(`${style(title, "bold")} ${style(`@${profile.handle}`, "cyan")}\n`);
  process.stdout.write(
    keyValues([
      ["id", profile.id],
      ["followers", formatCount(profile.followers)],
      ["bio", profile.bio === null ? undefined : profile.bio.replace(/\n+/g, " ")],
    ]) + "\n",
  );
  return 0;
}

export async function cmdThread(args: string[], json: boolean): Promise<number> {
  const id = args[0];
  if (id === undefined || id === "") {
    throw new Error("berikan ID tweet: xmot thread <id>");
  }
  const tweets = await xhlJson<Tweet[]>(["thread", id]);
  if (!json) process.stdout.write(heading(`Thread ${id}`) + "\n");
  renderTweets(tweets, json);
  return 0;
}

export async function cmdMetrics(args: string[], flags: Flags, json: boolean): Promise<number> {
  const id = args[0];
  if (id === undefined || id === "") {
    throw new Error("berikan ID tweet: xmot metrics <id>");
  }
  if (flagBool(flags, "history")) {
    const rows = await xhlJson<unknown[]>(["metrics", id, "--history", "--limit", String(limitOf(flags, 50))]);
    if (json) {
      printJson(rows);
    } else if (rows.length === 0) {
      process.stdout.write("belum ada snapshot; jalankan `xmot metrics <id>` dulu\n");
    } else {
      process.stdout.write(heading(`Riwayat metrics ${id}`) + "\n");
      for (const row of rows) process.stdout.write(`${JSON.stringify(row)}\n`);
    }
    return 0;
  }

  const metrics = await xhlJson<{
    likes: number | null;
    reposts: number | null;
    replies: number | null;
    views: number | null;
    bookmarks: number | null;
  }>(["metrics", id]);
  if (json) {
    printJson(metrics);
    return 0;
  }
  process.stdout.write(heading(`Metrics ${id}`) + "\n");
  process.stdout.write(
    keyValues([
      ["likes", formatCount(metrics.likes)],
      ["reposts", formatCount(metrics.reposts)],
      ["replies", formatCount(metrics.replies)],
      ["views", formatCount(metrics.views)],
      ["bookmarks", formatCount(metrics.bookmarks)],
    ]) + "\n",
  );
  return 0;
}

export async function cmdDoctor(json: boolean): Promise<number> {
  // xhl doctor memakai kode keluar sebagai hasil (0 sehat, 1 ada komponen
  // gagal), jadi kode itu diteruskan apa adanya — bukan dianggap kegagalan.
  if (json) return forwardXhl(["doctor", "--json"]);
  return forwardXhl(["doctor"]);
}

export async function cmdSessionStatus(json: boolean): Promise<number> {
  const result = await spawnXhl(["auth", "status", "--json"]);
  if (result.code !== 0) return forwardXhl(["auth", "status"]);
  if (json) {
    printJson(JSON.parse(result.stdout) as unknown);
    return 0;
  }
  process.stdout.write(result.stdout);
  return 0;
}

export async function cmdQueries(flags: Flags, json: boolean): Promise<number> {
  const refresh = flagBool(flags, "refresh");
  const argv = ["queries", refresh ? "refresh" : "list"];
  if (json) {
    return forwardXhl([...argv, "--json"]);
  }
  await xhlOrThrow(argv);
  return 0;
}