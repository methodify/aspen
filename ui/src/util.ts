/** decodeURIComponent that never throws: a malformed `%` sequence in a
 *  URL (a pasted or truncated link) comes back as written instead of
 *  crashing the console (the 2026-10 quality pass). */
export function safeDecode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}
