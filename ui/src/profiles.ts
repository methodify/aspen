// One console, several meshes (docs/PROPOSALS-2026-09-F.md). A profile is
// one mesh as this console sees it: the mesh name, this console's identity
// in that mesh, the connections that lead into it, and everything the
// browser remembers about that mesh (working set, cursors, drafts,
// preferences). One profile is active; switching reloads the page so
// every poll and the tunnel start over against the new mesh.
//
// Storage: every per-mesh key goes through `scoped()` and lands under
// `aspen.p.<id>.<key>`; browser-wide keys (theme, rail width, the
// profile list itself, the push sender key) stay bare. Served by a node
// the console is that node's window — one mesh — and `scoped()` is the
// identity function, so nothing changes there.
//
// The word "profile" never reaches the UI: a profile is shown by its
// mesh's name.

export const hosted: boolean = typeof __ASPEN_HOSTED__ !== "undefined" && __ASPEN_HOSTED__;

export interface Profile {
  id: string;
  /** The mesh's name; null until known (a direct node not asked yet, or
   *  an identity not certified yet). */
  mesh: string | null;
  /** Set when the node this profile reaches is in no mesh: the profile
   *  is then named by the node. */
  node?: string | null;
  /** Optional friendly label ("work", "home"). */
  label?: string;
  created: number;
}

const LIST_KEY = "aspen.profiles";
const ACTIVE_KEY = "aspen.profiles.active";
const PREFIX = "aspen.p.";

/** Keys that belong to the browser, not to a mesh: never scoped, never
 *  moved by the migration. */
const BROWSER_WIDE = new Set([
  "aspen.theme",
  "aspen.rail",
  "aspen.recapOnReturn",
  "aspen.session.renderMode",
  "aspen.push.sender",
  LIST_KEY,
  ACTIVE_KEY,
]);

function readList(): Profile[] {
  try {
    const raw = localStorage.getItem(LIST_KEY);
    return raw ? (JSON.parse(raw) as Profile[]) : [];
  } catch {
    return [];
  }
}
function writeList(list: Profile[]) {
  try {
    localStorage.setItem(LIST_KEY, JSON.stringify(list));
  } catch {
    /* storage unavailable */
  }
}

function newId(): string {
  return Math.random().toString(36).slice(2, 10);
}

let activeId: string | null = null;

/** The profile every scoped read and write goes to. Hosted, there is
 *  always one (made on first run, §2.7). */
export function activeProfile(): Profile | null {
  if (!hosted) return null;
  ensure();
  return readList().find((p) => p.id === activeId) ?? null;
}

export function listProfiles(): Profile[] {
  if (!hosted) return [];
  ensure();
  return readList();
}

/** `aspen.<key>` → `aspen.p.<active>.<key>` when hosted; unchanged when
 *  served by a node. */
export function scoped(key: string): string {
  if (!hosted) return key;
  ensure();
  return `${PREFIX}${activeId}.${key.startsWith("aspen.") ? key.slice(6) : key}`;
}

/** Another profile's copy of a key — read-only, for the cards and the
 *  push bookkeeping. */
export function readFrom(profileId: string, key: string): string | null {
  try {
    return localStorage.getItem(`${PREFIX}${profileId}.${key.startsWith("aspen.") ? key.slice(6) : key}`);
  } catch {
    return null;
  }
}

let ensured = false;
/** First run of a console that predates profiles: one profile holding
 *  everything it had, named by its cert's mesh when it has one. Bare
 *  per-mesh keys move under it, once. */
function ensure() {
  if (ensured) return;
  ensured = true;
  let list = readList();
  try {
    activeId = localStorage.getItem(ACTIVE_KEY);
  } catch {
    activeId = null;
  }
  if (list.length === 0) {
    let mesh: string | null = null;
    try {
      const raw = localStorage.getItem("aspen.console.identity");
      const id = raw ? (JSON.parse(raw) as { cert?: { mesh?: string } | null }) : null;
      mesh = id?.cert?.mesh ?? null;
    } catch {
      /* no identity */
    }
    const p: Profile = { id: newId(), mesh, created: Date.now() };
    list = [p];
    writeList(list);
    activeId = p.id;
    try {
      localStorage.setItem(ACTIVE_KEY, p.id);
    } catch {
      /* storage unavailable */
    }
    migrateBareKeys(p.id);
  }
  if (!activeId || !list.some((p) => p.id === activeId)) {
    activeId = list[0]!.id;
    try {
      localStorage.setItem(ACTIVE_KEY, activeId);
    } catch {
      /* storage unavailable */
    }
  }
}

function migrateBareKeys(pid: string) {
  try {
    const keys: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k && k.startsWith("aspen.") && !k.startsWith(PREFIX) && !BROWSER_WIDE.has(k)) keys.push(k);
    }
    for (const k of keys) {
      const v = localStorage.getItem(k);
      if (v !== null) localStorage.setItem(`${PREFIX}${pid}.${k.slice(6)}`, v);
      localStorage.removeItem(k);
    }
  } catch {
    /* storage unavailable */
  }
}

/** Fill in what the profile is named by, once known: the cert's mesh
 *  after certification, or the node's answer for a direct connection. */
export function setProfileMesh(mesh: string | null, node?: string | null, onlyIfUnknown = false) {
  if (!hosted) return;
  ensure();
  const list = readList();
  const p = list.find((x) => x.id === activeId);
  if (!p) return;
  if (onlyIfUnknown && p.mesh !== null) return;
  if (p.mesh === mesh && (node === undefined || p.node === node)) return;
  p.mesh = mesh;
  if (node !== undefined) p.node = node;
  writeList(list);
  for (const l of listeners) l();
}

export function setProfileLabel(id: string, label: string) {
  const list = readList();
  const p = list.find((x) => x.id === id);
  if (!p) return;
  p.label = label.trim() || undefined;
  writeList(list);
  for (const l of listeners) l();
}

/** How a profile is shown: its label, else its mesh, else the node it
 *  reaches, else "new mesh". */
export function profileName(p: Profile): string {
  return p.label ?? p.mesh ?? (p.node ? `${p.node} (no mesh)` : "new mesh");
}

/** A fresh profile — the way "connect to another mesh" begins. It
 *  becomes active and the page reloads onto the connect page, where the
 *  usual steps (identity, certify, relay and node, or a direct node)
 *  fill it in. */
export function addProfile(): never {
  ensure();
  const p: Profile = { id: newId(), mesh: null, created: Date.now() };
  const list = readList();
  list.push(p);
  writeList(list);
  switchTo(p.id, "/attach");
}

/** Routes that mean the same thing in every mesh; anything else (a
 *  session, a board, a view) names something in one mesh and is dropped
 *  on switch. */
const KEEP_ROUTES = new Set(["/", "/flow", "/mesh", "/history", "/search", "/boards", "/plugins", "/usage", "/attach"]);

function currentRoute(): string {
  // Hosted routes live in the hash (main.tsx).
  const h = window.location.hash.replace(/^#/, "");
  return h.split("?")[0] || "/";
}

/** Make `id` the active profile and reload, keeping the route when it
 *  is mesh-neutral. */
export function switchTo(id: string, route?: string): never {
  ensure();
  const list = readList();
  if (!list.some((p) => p.id === id)) throw new Error("no such mesh");
  try {
    localStorage.setItem(ACTIVE_KEY, id);
  } catch {
    /* storage unavailable */
  }
  const r = route ?? (KEEP_ROUTES.has(currentRoute()) ? currentRoute() : "/");
  window.location.hash = `#${r}`;
  window.location.reload();
  throw new Error("reloading");
}

/** Forget a mesh: every key under it, and the profile. Removing the
 *  active one switches to another (or a fresh one). */
export function removeProfile(id: string) {
  ensure();
  try {
    const prefix = `${PREFIX}${id}.`;
    const doomed: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k && k.startsWith(prefix)) doomed.push(k);
    }
    for (const k of doomed) localStorage.removeItem(k);
  } catch {
    /* storage unavailable */
  }
  void dropTranscripts(id);
  const list = readList().filter((p) => p.id !== id);
  writeList(list);
  if (activeId === id) {
    if (list.length === 0) {
      ensured = false;
      try {
        localStorage.removeItem(ACTIVE_KEY);
      } catch {
        /* storage unavailable */
      }
      window.location.hash = "#/attach";
      window.location.reload();
      return;
    }
    switchTo(list[0]!.id, "/attach");
  }
  for (const l of listeners) l();
}

/** The transcript cache (transcript.ts) keys rows by session name; a
 *  profile prefix keeps two meshes' same-named sessions apart. */
export function transcriptKey(name: string): string {
  if (!hosted) return name;
  ensure();
  return `${activeId}:${name}`;
}
async function dropTranscripts(pid: string): Promise<void> {
  const { dropTranscriptsWithPrefix } = await import("./transcript");
  await dropTranscriptsWithPrefix(`${pid}:`);
}

type Listener = () => void;
const listeners = new Set<Listener>();
export function onProfilesChange(l: Listener): () => void {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}

/** A push notification's link carries `mesh=<name>` (sw.ts): opened while
 *  another mesh is active, switch first, keeping the link's route. Runs
 *  before anything reads storage (main.tsx). */
export function followMeshLink(): void {
  if (!hosted) return;
  const h = window.location.hash.replace(/^#/, "");
  const q = h.indexOf("?");
  if (q < 0) return;
  const params = new URLSearchParams(h.slice(q + 1));
  const mesh = params.get("mesh");
  if (!mesh) return;
  params.delete("mesh");
  const rest = params.toString();
  const route = h.slice(0, q) + (rest ? `?${rest}` : "");
  ensure();
  const target = readList().find((p) => p.mesh === mesh);
  if (target && target.id !== activeId) {
    switchTo(target.id, route);
  }
  window.location.hash = `#${route}`;
}
