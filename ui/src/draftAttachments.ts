// A draft's attachments, kept with the draft (v0.51.2). The text lives in
// localStorage under `aspen.draft.<agent>`; the files are megabytes, so
// they go to IndexedDB under the same key. Leaving a session and coming
// back (another chat, another mesh, a reload) finds both where they were.

export interface DraftAttachment {
  n: number;
  name: string;
  media_type: string;
  /** base64, as the composer sends it */
  data: string;
  size: number;
}

const DB_NAME = "aspen-drafts";
const STORE = "attachments";

function openDb(): Promise<IDBDatabase | null> {
  return new Promise((resolve) => {
    try {
      const req = indexedDB.open(DB_NAME, 1);
      req.onupgradeneeded = () => {
        if (!req.result.objectStoreNames.contains(STORE)) req.result.createObjectStore(STORE);
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => resolve(null);
    } catch {
      resolve(null);
    }
  });
}

export async function loadDraftAttachments(key: string): Promise<DraftAttachment[]> {
  const db = await openDb();
  if (!db) return [];
  return new Promise((resolve) => {
    try {
      const req = db.transaction(STORE, "readonly").objectStore(STORE).get(key);
      req.onsuccess = () => {
        const v = req.result as { items?: DraftAttachment[] } | undefined;
        resolve(Array.isArray(v?.items) ? v.items : []);
      };
      req.onerror = () => resolve([]);
    } catch {
      resolve([]);
    }
  });
}

/** Save (or, with none, forget) a draft's attachments. A failed write
 *  (quota) leaves them in memory only, as before. */
export async function saveDraftAttachments(key: string, items: DraftAttachment[]): Promise<void> {
  const db = await openDb();
  if (!db) return;
  await new Promise<void>((resolve) => {
    try {
      const tx = db.transaction(STORE, "readwrite");
      const store = tx.objectStore(STORE);
      if (items.length) store.put({ items, at: Date.now() }, key);
      else store.delete(key);
      tx.oncomplete = () => resolve();
      tx.onerror = () => resolve();
      tx.onabort = () => resolve();
    } catch {
      resolve();
    }
  });
}

/** The attachments the draft text still has a marker for. */
export function stillMarked<T extends { n: number }>(items: T[], draft: string): T[] {
  return items.filter((a) => draft.includes(`[attachment ${a.n}:`));
}
