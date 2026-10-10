/* Getting an agent's file out of the console (PROPOSALS-2026-10-R.md R-3):
 * copy, share, drag, and an Aspen file reference that any Aspen composer
 * turns back into an attachment. A web page cannot put an arbitrary file
 * on the system clipboard (browsers allow text, HTML and PNG), so copy
 * carries what each kind allows: a PNG for images, the text for text
 * files, the path otherwise, and the reference in HTML for Aspen. */
import { api } from "./api";

/** What a reference names: the agent (qualified with its node, so another
 *  console can find it) and the path as that agent's node serves it. */
export interface AspenFileRef {
  agent: string;
  path: string;
  name: string;
  media: string;
}

/** `bare@repo` plus the node, when the name has none yet. */
export function qualifiedAgent(agent: string, node: string | null | undefined): string {
  return agent.split("@").length === 2 && node ? `${agent}@${node}` : agent;
}

const isRaster = (m: string) => /^image\/(png|jpeg|gif|webp)$/i.test(m);
export const isTextLike = (m: string) => m.startsWith("text/") || /json|xml|javascript|ndjson|yaml|toml|csv/i.test(m);
const TEXT_COPY_MAX = 1024 * 1024;

function escapeAttr(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}

/** The reference as HTML: harmless where pasted elsewhere (it reads as the
 *  file's name), recognised by an Aspen composer. */
export function refHtml(ref: AspenFileRef): string {
  return `<span data-aspen-file="${escapeAttr(JSON.stringify(ref))}">${escapeAttr(ref.name)}</span>`;
}

/** The reference in pasted or dropped HTML, if any. */
export function parseRef(html: string | null | undefined): AspenFileRef | null {
  if (!html || !html.includes("data-aspen-file")) return null;
  try {
    const doc = new DOMParser().parseFromString(html, "text/html");
    const raw = doc.querySelector("[data-aspen-file]")?.getAttribute("data-aspen-file");
    const v = raw ? (JSON.parse(raw) as AspenFileRef) : null;
    return v && typeof v.agent === "string" && typeof v.path === "string" ? v : null;
  } catch {
    return null;
  }
}

/** The file a reference names, fetched through this console's node (the
 *  relay or direct, as for the viewer). A name qualified with this very
 *  node is also tried without it. */
export async function refToFile(ref: AspenFileRef, selfNode?: string | null): Promise<File> {
  let blob: Blob;
  try {
    blob = await api.fileBlob(ref.agent, ref.path);
  } catch (e) {
    const parts = ref.agent.split("@");
    if (parts.length === 3 && selfNode && parts[2] === selfNode) {
      blob = await api.fileBlob(`${parts[0]}@${parts[1]}`, ref.path);
    } else {
      throw e;
    }
  }
  return new File([blob], ref.name, { type: ref.media || blob.type });
}

async function toPng(blob: Blob): Promise<Blob> {
  if (blob.type === "image/png") return blob;
  const bmp = await createImageBitmap(blob);
  const c = document.createElement("canvas");
  c.width = bmp.width;
  c.height = bmp.height;
  c.getContext("2d")!.drawImage(bmp, 0, 0);
  return await new Promise<Blob>((res, rej) => c.toBlob((b) => (b ? res(b) : rej(new Error("png encode failed"))), "image/png"));
}

/** Copy: what the clipboard can carry for this kind of file. Returns what
 *  was copied, for the caller's note. Must run inside the click (Safari):
 *  the clipboard item is built from promises that resolve afterwards. */
export async function copyFile(ref: AspenFileRef, size: number | null): Promise<string> {
  const blobP = api.fileBlob(ref.agent, ref.path);
  const html = new Blob([refHtml(ref)], { type: "text/html" });
  const textLike = isTextLike(ref.media) && (size ?? 0) <= TEXT_COPY_MAX;
  const plain: Promise<Blob> = textLike
    ? blobP.then(async (b) => new Blob([await b.text()], { type: "text/plain" }))
    : Promise.resolve(new Blob([ref.path], { type: "text/plain" }));
  const items: Record<string, Promise<Blob> | Blob> = { "text/html": html, "text/plain": plain };
  if (isRaster(ref.media)) items["image/png"] = blobP.then(toPng);
  if (typeof ClipboardItem !== "undefined" && navigator.clipboard?.write) {
    await navigator.clipboard.write([new ClipboardItem(items)]);
    return isRaster(ref.media) ? "the image" : textLike ? "its text" : "its path";
  }
  await navigator.clipboard.writeText(ref.path);
  return "its path";
}

/** Can this browser hand files to the system share sheet? */
export function canShareFiles(): boolean {
  try {
    return typeof navigator.share === "function" && !!navigator.canShare?.({ files: [new File([""], "x.txt", { type: "text/plain" })] });
  } catch {
    return false;
  }
}

/** Share: the file itself, to any app (the Claude app, Mail, AirDrop…). */
export async function shareFile(ref: AspenFileRef): Promise<void> {
  const blob = await api.fileBlob(ref.agent, ref.path);
  const file = new File([blob], ref.name, { type: ref.media || blob.type });
  await navigator.share({ files: [file], title: ref.name });
}

/** Drag: the reference for an Aspen composer, the path as text, and (in
 *  Chrome, when the file has a direct URL) a file the desktop accepts. */
export function onFileDragStart(e: React.DragEvent, ref: AspenFileRef) {
  e.dataTransfer.effectAllowed = "copy";
  e.dataTransfer.setData("text/html", refHtml(ref));
  e.dataTransfer.setData("text/plain", ref.path);
  if (!api.filesViaTunnel()) {
    const url = new URL(api.fileUrl(ref.agent, ref.path, { download: true }), window.location.href).href;
    e.dataTransfer.setData("DownloadURL", `${ref.media || "application/octet-stream"}:${ref.name}:${url}`);
  }
}
