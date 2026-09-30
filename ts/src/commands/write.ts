/**
 * Perintah tulis: post, thread, schedule, draft.
 *
 * Aturan yang berlaku di seluruh berkas ini: **tidak ada yang terkirim ke X
 * tanpa persetujuan eksplisit**. Posting tidak dapat dibatalkan, jadi setiap
 * perintah menampilkan teks final lebih dulu lalu membaca konfirmasi.
 *
 * Isi tweet dikirim lewat **berkas sementara**, bukan argumen `--file -`, karena
 * membaca stdin xhl adalah jalur yang sama-sama bergantung pada pipe dan sudah
 * terbukti rapuh di lingkungan headless.
 */

import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { xhlJson } from "../runner.ts";
import { heading, keyValues, printJson, style } from "../ui.ts";
import { flagBool, flagStr, type Flags } from "./read.ts";

type Posted = { id: string; url: string; posted_at: string };

/**
 * Tulis teks ke berkas sementara dan kembalikan **path**-nya.
 *
 * xhl menerima path filesystem apa adanya (`--file <path>`), bukan URL
 * `file://` — mengirim URL membuat xhl melaporkan "konten tidak ditemukan".
 */
async function writeTempContent(
  text: string,
): Promise<{ arg: string; cleanup: () => Promise<void> }> {
  const dir = await mkdtemp(join(tmpdir(), "xmot-"));
  const path = join(dir, "content.txt");
  await writeFile(path, text, { encoding: "utf8", mode: 0o600 });
  return {
    arg: path,
    cleanup: async () => {
      // Dihapus apa pun hasilnya: isi tweet tidak boleh tertinggal di /tmp.
      await rm(dir, { recursive: true, force: true }).catch(() => {});
    },
  };
}

/**
 * Minta konfirmasi.
 *
 * Argumen `--yes` melewati pertanyaan agar pemakaian dari skrip tetap mungkin;
 * itu tetap keputusan eksplisit, bukan default implisit.
 */
async function confirm(prompt: string, accepted: boolean): Promise<boolean> {
  if (accepted) return true;
  if (!process.stdin.isTTY) {
    // Tanpa terminal tidak ada yang bisa menjawab; berhenti daripada menggantung.
    throw new Error(
      "konfirmasi diperlukan tetapi stdin bukan terminal. Tambahkan --yes bila memang ingin mengirim.",
    );
  }
  process.stderr.write(`${prompt} ${style("[y/N]", "dim")} `);
  for await (const chunk of Bun.stdin.stream()) {
    const answer = new TextDecoder().decode(chunk).trim().toLowerCase();
    return answer === "y" || answer === "ya" || answer === "yes";
  }
  return false;
}

/** Baca isi dari `--file` (mendukung `-` = stdin), atau `--text`. */
export async function resolveContent(flags: Flags): Promise<string> {
  const file = flagStr(flags, "file");
  if (file !== undefined && file !== "") {
    if (file === "-") {
      const text = await new Response(Bun.stdin.stream()).text();
      return text;
    }
    const raw = await Bun.file(file).text();
    return raw;
  }
  const text = flagStr(flags, "text");
  if (text !== undefined && text !== "") return text;
  throw new Error("berikan isi: --text \"...\" atau --file <path> (atau - untuk stdin)");
}

function previewContent(text: string): string {
  const body = text.trim();
  return `${style("─".repeat(60), "dim")}\n${body}\n${style("─".repeat(60), "dim")}\n` +
    `${style(`${body.length} karakter`, "dim")}`;
}

export async function cmdPost(flags: Flags, json: boolean): Promise<number> {
  const text = await resolveContent(flags);
  if (text.trim() === "") throw new Error("isi tweet kosong");

  const media = collectValues(flags, "media");
  process.stdout.write(heading("Akan dikirim ke X") + "\n");
  process.stdout.write(`${previewContent(text)}\n`);
  if (media.length > 0) {
    process.stdout.write(`${style("Media:", "dim")} ${media.join(", ")}\n`);
  }

  if (!(await confirm("Kirim sekarang?", flagBool(flags, "yes")))) {
    process.stdout.write("dibatalkan; tidak ada yang terkirim\n");
    return 1;
  }

  const tmp = await writeTempContent(text);
  try {
    const argv = ["post", "--file", tmp.arg];
    for (const m of media) argv.push("--media", m);
    const replyTo = flagStr(flags, "reply-to");
    if (replyTo !== undefined && replyTo !== "") argv.push("--reply-to", replyTo);

    const posted = await xhlJson<Posted>(argv);
    if (json) {
      printJson(posted);
    } else {
      process.stdout.write(`${style("terkirim", "green")} ${posted.url}\n`);
      process.stdout.write(keyValues([["id", posted.id]]) + "\n");
    }
    return 0;
  } finally {
    await tmp.cleanup();
  }
}

/** Kumpulkan nilai flag yang berulang (`--media a --media b`). */
function collectValues(flags: Flags, name: string): string[] {
  const raw = flags[name];
  if (raw === undefined || raw === false) return [];
  if (raw === true) return [];
  return raw.split("\0").filter((s) => s !== "");
}

/**
 * Kirim utas.
 *
 * Bagian dipisah baris kosong dan dirantai oleh `xhl post --reply-to`, memakai
 * ID yang dikembalikan tiap langkah. Berhenti pada bagian pertama yang gagal —
 * melanjutkan akan menghasilkan utas yang terputus di tengah tanpa jejak.
 */
export async function cmdThreadPost(flags: Flags, json: boolean): Promise<number> {
  const text = await resolveContent(flags);
  const parts = text
    .split(/\n\s*\n/)
    .map((p) => p.trim())
    .filter((p) => p !== "");
  if (parts.length === 0) throw new Error("isi utas kosong");

  const skipped = parts.filter((p) => p.length > 280);
  if (skipped.length > 0) {
    // xhl akan menerimanya, tetapi panjang berlebih biasanya pertanda bagian
    // belum terpisah dengan benar. Ditahan di sini, bukan setelah terkirim.
    throw new Error(
      `ada ${skipped.length} bagian melebihi 280 karakter — pisahkan dengan baris kosong`,
    );
  }

  process.stdout.write(heading(`Akan dikirim sebagai utas (${parts.length} bagian)`) + "\n");
  parts.forEach((p, i) => {
    process.stdout.write(`${style(`${i + 1}/${parts.length}`, "dim")} ${p}\n`);
  });

  if (!(await confirm("Kirim seluruh utas?", flagBool(flags, "yes")))) {
    process.stdout.write("dibatalkan; tidak ada yang terkirim\n");
    return 1;
  }

  const posted: Posted[] = [];
  let replyTo: string | undefined;
  for (const [i, part] of parts.entries()) {
    const tmp = await writeTempContent(part);
    try {
      const argv: string[] = ["post", "--file", tmp.arg];
      if (replyTo !== undefined) argv.push("--reply-to", replyTo);
      const result = await xhlJson<Posted>(argv);
      posted.push(result);
      replyTo = result.id;
      process.stdout.write(
        `${style(`${i + 1}/${parts.length}`, "green")} ${result.url}\n`,
      );
    } catch (err) {
      const done = posted.length;
      throw new Error(
        `utas berhenti di bagian ${i + 1} (${done} bagian sudah terkirim). ` +
          `Sebab: ${err instanceof Error ? err.message : String(err)}`,
      );
    } finally {
      await tmp.cleanup();
    }
  }

  if (json) printJson(posted);
  return 0;
}

export async function cmdSchedule(flags: Flags, json: boolean): Promise<number> {
  const at = flagStr(flags, "at");
  if (at === undefined || at === "") {
    throw new Error("berikan waktu: --at 2026-10-01T09:00:00+07:00");
  }
  const text = await resolveContent(flags);
  const parsed = Date.parse(at);
  if (Number.isNaN(parsed)) {
    throw new Error(`--at bukan waktu yang dapat dibaca: ${at}`);
  }

  process.stdout.write(heading("Akan dijadwalkan") + "\n");
  process.stdout.write(`${previewContent(text)}\n`);
  process.stdout.write(`${style("Waktu:", "dim")} ${new Date(parsed).toISOString()}\n`);
  // Jadwal eksekusinya bergantung pada daemon; pengguna harus tahu itu sekarang,
  // bukan menyimpulkan "tidak jalan" setelah waktunya lewat.
  process.stdout.write(
    `${style("Catatan:", "yellow")} job baru terkirim bila daemon \`xhl run\` sedang berjalan\n`,
  );

  if (!(await confirm("Jadwalkan?", flagBool(flags, "yes")))) {
    process.stdout.write("dibatalkan\n");
    return 1;
  }

  const tmp = await writeTempContent(text);
  try {
    const result = await xhlJson<{ id?: string }>([
      "schedule",
      "post",
      "--at",
      at,
      "--file",
      tmp.arg,
    ]);
    if (json) printJson(result);
    else process.stdout.write(`${style("terjadwal", "green")} ${result.id ?? ""}\n`);
    return 0;
  } finally {
    await tmp.cleanup();
  }
}

export async function cmdScheduleList(json: boolean): Promise<number> {
  const rows = await xhlJson<unknown[]>(["schedule", "list", "--limit", "50"]);
  if (json) {
    printJson(rows);
    return 0;
  }
  if (rows.length === 0) {
    process.stdout.write("belum ada job terjadwal\n");
    return 0;
  }
  for (const row of rows) process.stdout.write(`${JSON.stringify(row)}\n`);
  return 0;
}

export async function cmdDraft(args: string[], flags: Flags, json: boolean): Promise<number> {
  const action = args[0] ?? "list";
  switch (action) {
    case "list": {
      const rows = await xhlJson<unknown[]>(["draft", "list", "--limit", "50"]);
      if (json) printJson(rows);
      else if (rows.length === 0) process.stdout.write("belum ada draft\n");
      else for (const row of rows) process.stdout.write(`${JSON.stringify(row)}\n`);
      return 0;
    }
    case "new": {
      const text = await resolveContent(flags);
      const result = await xhlJson<{ id?: string }>(["draft", "new", "--body", text]);
      if (json) printJson(result);
      else process.stdout.write(`${style("draft tersimpan", "green")} ${result.id ?? ""}\n`);
      return 0;
    }
    case "post": {
      const id = args[1];
      if (id === undefined || id === "") throw new Error("berikan ID draft: xmot draft post <id>");
      const result = await xhlJson<Posted>(["draft", "post", id]);
      if (json) printJson(result);
      else process.stdout.write(`${style("terkirim", "green")} ${result.url}\n`);
      return 0;
    }
    default:
      throw new Error(`aksi draft tidak dikenal: ${action} (pakai list/new/post)`);
  }
}