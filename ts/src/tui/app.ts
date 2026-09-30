/**
 * TUI untuk xhl.
 *
 * Struktur layar: daftar tugas di kiri (Select), isi di kanan (ScrollBox berisi
 * teks). Hasil perintah **teks** diambil dari `xhl` apa adanya (`--driver fake`
 * pun jalan), kecuali perintah yang butuh struktur (cari/timeline/trend/profil)
 * yang memakai `--json` agar dapat diformat rapi.
 *
 * Setiap pemanggilan xhl adalah proses terpisah: TUI tidak menahan koneksi atau
 * sesi sendiri, sehingga perilakunya identik dengan CLI.
 */

import {
  BoxRenderable,
  InputRenderable,
  InputRenderableEvents,
  ScrollBoxRenderable,
  SelectRenderable,
  SelectRenderableEvents,
  TextRenderable,
  createCliRenderer,
  type CliRenderer,
  type KeyEvent,
} from "@opentui/core";

import { spawnXhl, xhlJson, XhlError } from "../runner.ts";
import type { TimelinePage, Trend, Tweet, UserProfile } from "../types.ts";
import {
  doctorLines,
  errorLines,
  metricsLines,
  profileLines,
  trendLines,
  tweetLines,
} from "./views.ts";

/** Satu entri di daftar tugas. */
type Task = {
  id: string;
  name: string;
  description: string;
};

const TASKS: Task[] = [
  { id: "doctor", name: "Kesehatan sesi", description: "cookie, signer, queryId" },
  { id: "trends", name: "Trend", description: "topik yang sedang ramai" },
  { id: "search", name: "Cari tweet", description: "butuh kata kunci di baris perintah" },
  { id: "timeline", name: "Timeline", description: "home / terbaru" },
  { id: "user", name: "Profil pengguna", description: "butuh handle" },
  { id: "metrics", name: "Metrics tweet", description: "butuh ID tweet" },
  { id: "queries", name: "Query ID", description: "daftar cache discovery" },
];

/** Warna bertema gelap; sengaja sedikit agar terbaca di terminal apa pun. */
const C = {
  bg: "#14161c",
  panel: "#1b1e26",
  accent: "#7aa2f7",
  dim: "#565f89",
  ok: "#9ece6a",
  warn: "#e0af68",
  err: "#f7768e",
  fg: "#c0caf5",
};

export type Ui = {
  renderer: CliRenderer;
  taskList: SelectRenderable;
  argInput: InputRenderable;
  output: TextRenderable;
  scroll: ScrollBoxRenderable;
  status: TextRenderable;
  outputLines: string[];
};

/** Bangun pohon komponen. Dipisah agar `state` tidak bercampur dengan tata letak. */
export function buildUi(renderer: CliRenderer): Ui {
  const root = new BoxRenderable(renderer, {
    id: "root",
    width: "100%",
    height: "100%",
    flexDirection: "column",
    backgroundColor: C.bg,
  });

  const header = new TextRenderable(renderer, {
    id: "header",
    content: "xmot — X Headless",
    fg: C.accent,
    height: 1,
  });

  const body = new BoxRenderable(renderer, {
    id: "body",
    width: "100%",
    flexGrow: 1,
    flexDirection: "row",
    gap: 1,
  });

  const left = new BoxRenderable(renderer, {
    id: "left",
    width: 34,
    height: "100%",
    flexDirection: "column",
    backgroundColor: C.panel,
    padding: 1,
  });
  left.add(new TextRenderable(renderer, { content: "TUGAS", fg: C.dim, height: 1 }));

  const taskList = new SelectRenderable(renderer, {
    id: "tasks",
    width: "100%",
    flexGrow: 1,
    options: TASKS.map((t) => ({ name: t.name, description: t.description, value: t.id })),
    showDescription: true,
    showScrollIndicator: true,
    wrapSelection: true,
  });
  left.add(taskList);

  const right = new BoxRenderable(renderer, {
    id: "right",
    flexGrow: 1,
    height: "100%",
    flexDirection: "column",
    backgroundColor: C.panel,
    padding: 1,
  });
  const output = new TextRenderable(renderer, {
    id: "output",
    content: "\n  Pilih tugas di kiri, lalu Enter.",
    fg: C.fg,
    width: "100%",
  });
  const scroll = new ScrollBoxRenderable(renderer, {
    id: "scroll",
    width: "100%",
    flexGrow: 1,
    scrollY: true,
  });
  scroll.add(output);
  right.add(scroll);

  body.add(left);
  body.add(right);

  const argInput = new InputRenderable(renderer, {
    id: "arg",
    width: "100%",
    placeholder: "argumen (kata kunci / handle / id) — Enter untuk jalankan",
    backgroundColor: C.bg,
    textColor: C.fg,
    placeholderColor: C.dim,
    focusedBackgroundColor: C.bg,
    focusedTextColor: C.fg,
  });

  const status = new TextRenderable(renderer, {
    id: "status",
    content: "siap",
    fg: C.dim,
    height: 1,
  });

  root.add(header);
  root.add(body);
  root.add(argInput);
  root.add(status);
  renderer.root.add(root);

  return { renderer, taskList, argInput, output, scroll, status, outputLines: [] };
}

/** Ganti isi panel kanan dan gulir ke atas. */
export function show(ui: Ui, lines: string[]): void {
  ui.outputLines = lines;
  ui.output.content = lines.join("\n");
  ui.scroll.scrollTo({ x: 0, y: 0 });
  ui.renderer.requestRender();
}

export function setStatus(ui: Ui, text: string, color: string = C.dim): void {
  ui.status.content = text;
  ui.status.fg = color;
  ui.renderer.requestRender();
}

/** Argumen default yang masuk akal per tugas; mengurangi pengetikan. */
function defaultArgFor(taskId: string): string {
  switch (taskId) {
    case "timeline":
      return "home";
    case "trends":
      return "trending";
    default:
      return "";
  }
}

export async function runTask(ui: Ui, taskId: string, arg: string): Promise<void> {
  setStatus(ui, `menjalankan ${taskId}…`, C.warn);
  show(ui, ["", "  memuat…"]);
  try {
    switch (taskId) {
      case "doctor": {
        const { stdout, code } = await spawnXhl(["doctor"]);
        const lines = doctorLines(stdout);
        // Kode 1 dari `doctor` berarti ada komponen gagal — itu hasil, bukan error.
        setStatus(ui, code === 0 ? "sesi sehat" : "ada komponen gagal", code === 0 ? C.ok : C.err);
        show(ui, [code === 0 ? "" : "", `  ${code === 0 ? "SEHAT" : "PERHATIAN"}`, ...lines]);
        return;
      }
      case "queries": {
        const { stdout } = await spawnXhl(["queries", "list"]);
        show(ui, ["", ...stdout.split("\n").filter((l) => l !== "").map((l) => `  ${l}`)]);
        setStatus(ui, "daftar query ID");
        return;
      }
      case "trends": {
        const category = arg.trim() === "" ? "trending" : arg.trim();
        const trends = await xhlJson<Trend[]>(["trends", "--category", category, "--limit", "25"]);
        show(ui, trendLines(trends));
        setStatus(ui, `${trends.length} trend (${category})`);
        return;
      }
      case "search": {
        if (arg.trim() === "") {
          setStatus(ui, "tulis kata kunci dulu di baris bawah", C.warn);
          show(ui, ["", "  Kata kunci kosong.", "  Contoh: rust", "  Tambahkan --top untuk yang paling rame."]);
          return;
        }
        const top = arg.includes("--top");
        const query = arg.replace("--top", "").trim();
        const argv = ["search", query, "--limit", "20"];
        if (top) argv.push("--top");
        const tweets = await xhlJson<Tweet[]>(argv);
        show(ui, tweetLines(tweets, `${query}${top ? " — paling rame" : " — terbaru"}`));
        setStatus(ui, `${tweets.length} tweet`);
        return;
      }
      case "timeline": {
        const kind = arg.trim() === "" ? "home" : arg.trim();
        const page = await xhlJson<TimelinePage>(["timeline", "--kind", kind, "--limit", "20"]);
        show(ui, tweetLines(page.items, `timeline: ${kind}`));
        setStatus(ui, `${page.items.length} tweet`);
        return;
      }
      case "user": {
        if (arg.trim() === "") {
          setStatus(ui, "tulis handle dulu, mis. rustlang", C.warn);
          show(ui, ["", "  Handle kosong."]);
          return;
        }
        const profile = await xhlJson<UserProfile>(["user", "--handle", arg.trim().replace(/^@/, "")]);
        show(ui, profileLines(profile));
        setStatus(ui, `@${profile.handle}`);
        return;
      }
      case "metrics": {
        if (arg.trim() === "") {
          setStatus(ui, "tulis ID tweet dulu", C.warn);
          show(ui, ["", "  ID tweet kosong."]);
          return;
        }
        const id = arg.trim();
        const m = await xhlJson<{
          likes: number | null;
          reposts: number | null;
          replies: number | null;
          views: number | null;
          bookmarks: number | null;
        }>(["metrics", id]);
        show(ui, metricsLines(id, m));
        setStatus(ui, "snapshot tersimpan");
        return;
      }
      default:
        show(ui, ["", `  tugas tidak dikenal: ${taskId}`]);
        setStatus(ui, "tugas tidak dikenal", C.err);
    }
  } catch (err) {
    // Pesan dari xhl diteruskan apa adanya; menyembunyikannya akan membuat
    // pengguna mencari sebab di tempat yang salah.
    const msg = err instanceof XhlError ? err.message : err instanceof Error ? err.message : String(err);
    const hint = err instanceof XhlError ? err.hint : undefined;
    setStatus(ui, "gagal", C.err);
    show(ui, errorLines(msg, hint));
  }
}

/** Jalankan TUI hingga pengguna menekan q atau Ctrl+C. */
export async function startTui(): Promise<void> {
  const renderer = await createCliRenderer({
    exitOnCtrlC: true,
    backgroundColor: C.bg,
  });
  const ui = buildUi(renderer);

  ui.taskList.on(SelectRenderableEvents.ITEM_SELECTED, (_index, option) => {
    const taskId = String(option.value);
    const arg = ui.argInput.value.trim() === "" ? defaultArgFor(taskId) : ui.argInput.value;
    ui.argInput.value = arg;
    void runTask(ui, taskId, arg);
  });

  // Enter di baris argumen menjalankan tugas yang sedang disorot.
  ui.argInput.on(InputRenderableEvents.ENTER, () => {
    const selected = ui.taskList.getSelectedOption();
    if (selected === null) return;
    void runTask(ui, String(selected.value), ui.argInput.value);
  });

  renderer.keyInput.on("keypress", (key: KeyEvent) => {
    if (key.name === "q") {
      renderer.destroy();
      return;
    }
    // Tab memindahkan fokus antara daftar tugas dan baris argumen.
    if (key.name === "tab") {
      const focused = renderer.currentFocusedRenderable;
      if (focused === ui.argInput) ui.taskList.focus();
      else ui.argInput.focus();
      renderer.requestRender();
    }
  });

  ui.taskList.focus();
  renderer.requestRender();

  // Muat status sesi lebih dulu: itu pertanyaan pertama setiap orang, dan
  // jawabannya menjelaskan mengapa perintah lain mungkin gagal.
  await runTask(ui, "doctor", "");
}