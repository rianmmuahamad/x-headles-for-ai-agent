/**
 * Bentuk data yang dikirim `xhl --json`.
 *
 * Ini kontrak dengan `xhl-core` (lihat `crates/xhl-core/src/domain.rs`).
 *
 * Catatan penting: serde menulis `Option::None` sebagai **`null`**, bukan
 * menghilangkan field-nya. Jadi field opsional di sini bertipe `| null`, dan
 * formatter wajib menangani `null` — memperlakukannya sebagai `undefined`
 * membuat nilai kosong tampil sebagai teks "null" di layar.
 */

export type Handle = string;

export type Author = {
  handle: Handle;
  display_name: string;
  verified: boolean;
};

export type Metrics = {
  likes: number | null;
  reposts: number | null;
  replies: number | null;
  views: number | null;
  bookmarks: number | null;
};

export type MediaItem = {
  kind: "image" | "video" | "gif";
  url: string;
  thumbnail_url: string | null;
  alt: string | null;
};

export type Tweet = {
  id: string;
  author: Author;
  text: string;
  /** RFC3339. */
  created_at: string;
  media: MediaItem[];
  metrics: Metrics | null;
  url: string;
};

export type UserProfile = {
  id: string;
  handle: Handle;
  display_name: string;
  verified: boolean;
  bio: string | null;
  followers: number | null;
};

export type Trend = {
  name: string;
  rank: number | null;
  context: string | null;
  meta_description: string | null;
  url: string | null;
};

export type TimelinePage = {
  items: Tweet[];
  cursor: string | null;
};