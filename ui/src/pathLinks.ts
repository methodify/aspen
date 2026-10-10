// Paths the agent writes in prose become links to the viewer
// (PROPOSALS-2026-09 §3). Detection is textual and optimistic: a token
// that looks like a file path is linked; opening one that isn't there
// says so. Fenced and inline code are left alone.

/** The viewer route for `path` as seen by `agent`'s home node. */
export function viewHref(agent: string, path: string): string {
  return `/view/${encodeURIComponent(agent)}?path=${encodeURIComponent(path)}`;
}

// A path: absolute POSIX, home-relative, Windows drive, or file://; must
// contain a dot-extension or end in a known extensionless filename, and
// must not run into quotes/brackets/backticks.
const PATH_RE =
  /(^|[\s(\[,:;])((?:file:\/\/)?(?:~\/|\/|[A-Za-z]:[\\/])[^\s"'`<>()\[\]]+?(?:\.[A-Za-z0-9]{1,8}|\/(?:Makefile|Dockerfile|LICENSE|README))(?=[\s.,;:)\]]|$))/g;

// Repo-relative paths: two or more segments (or one with a known
// extension) and no leading scheme; kept conservative to avoid linking
// prose like "and/or".
const REL_RE =
  /(^|[\s(\[,:;])((?:[A-Za-z0-9_.-]+\/)+[A-Za-z0-9_.-]+\.(?:md|txt|rs|ts|tsx|js|jsx|py|json|jsonl|toml|yml|yaml|css|html|png|jpg|jpeg|gif|webp|svg|pdf|csv|log|sh))(?=[\s.,;:)\]]|$)/g;

/**
 * Rewrite path tokens outside code spans/fences into markdown links.
 * Trailing punctuation is left outside the link.
 */
export function linkifyPaths(text: string, agent: string): string {
  if (!text) return text;
  // Split on fenced blocks; only transform the prose parts.
  const parts = text.split(/(```[\s\S]*?```)/g);
  return parts
    .map((part, i) => {
      if (i % 2 === 1) return part; // a fence
      // Within prose, skip inline code spans and existing markdown
      // links/images (their targets must stay as written).
      const spans = part.split(/(`[^`\n]*`|!?\[[^\]\n]*\]\([^)\n]*\))/g);
      return spans
        .map((sp, j) => {
          if (j % 2 === 1) return sp;
          let out = sp.replace(PATH_RE, (_m, lead: string, p: string) => {
            const clean = p.replace(/^file:\/\//, "");
            return `${lead}[${p}](${viewHref(agent, clean)})`;
          });
          out = out.replace(REL_RE, (_m, lead: string, p: string) => {
            // Don't double-link something PATH_RE already wrapped.
            return `${lead}[${p}](${viewHref(agent, p)})`;
          });
          return out;
        })
        .join("");
    })
    .join("");
}

/** The path an inline code span names, when the whole span is one: an
 *  absolute, home-relative or drive path, or a repo-relative file with a
 *  known extension (`docs/report.md`, `notes.md`). Agents put paths in
 *  backticks far more often than in prose, and those were never linked
 *  (PROPOSALS-2026-10-R.md R-4). */
export function codePathTarget(code: string): string | null {
  const s = code.trim();
  if (!s || /\s/.test(s) || s.length > 400) return null;
  if (/^(?:file:\/\/)?(?:~\/|\/|[A-Za-z]:[\\/])[^"'`<>()[\]]+$/.test(s) && /[./]/.test(s.slice(1))) {
    return s.replace(/^file:\/\//, "");
  }
  if (/^(?:[A-Za-z0-9_.-]+\/)*[A-Za-z0-9_.-]+\.(?:md|txt|rs|ts|tsx|js|jsx|py|json|jsonl|toml|yml|yaml|css|html|png|jpg|jpeg|gif|webp|svg|pdf|csv|log|sh|sql|xml|docx|xlsx|pptx|zip)$/.test(s)) {
    return s;
  }
  return null;
}
