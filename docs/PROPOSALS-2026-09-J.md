# Proposal: names, transcripts and the branch verb

**Status:** decided 2026-09-21 (§8); S-1..S-4 and S-7 shipped as v0.36;
S-5, S-6, S-8 in the backlog. References: DESIGN.md log 2026-09-04
(adoption), `crates/aspen-node/src/node.rs` (`branch_agent`,
`split_agent`, `fork_to`), `adoption.rs`, `ui/src/pages/Library.tsx`
(the Mesh list), `ui/src/pages/Session.tsx` (the branch dialog, the
bookmarks panel).

## 0. The report

Working in `@dw-optimize`, the operator typed `/branch`. The branch took
on the `dw-optimize` identity, which was not the intent. Going to the
repo in the Mesh list to sort it out, the sessions there are labelled by
words that appear nowhere in the session views, so there is no telling
which transcript is which, and nothing on the page says what
`@dw-optimize` points at or lets it be pointed elsewhere.

Earlier, the ⎇ button: an error, and the session the operator thought
they had branched into was nowhere to be found; the old session
continued under its identity. They branched from the TUI instead.

## 1. What the design says, and why

The 2026-09-04 decision (adoption) fixed the model: an **agent is a name
that points at a head session**; branching forks the transcript and
asks *which name follows the fork*. Two answers: **carry** (the name
moves to the fork; the tip it leaves is written down as a *bookmark*)
and **split** (a new name takes the fork; this name stays). Bookmarks
can be resumed later, again as carry or split. The runtime has no
branch signal, so forks made outside Aspen are noticed from disk and
raised as *needs* with the same two verbs plus ignore; "a name never
moves on its own."

The reasoning was: the name is the durable thing (the bus address, the
board slot, the history), the transcript is a line under it, and the
operator should be able to try something on a fork and come back. That
holds. What was never decided is the **default**, the **place**, and
the **words**:

- `POST /agents/{name}/branch` without `as` is carry. So is `/branch`
  with no arguments, and the ⎇ dialog when its "as new agent" field is
  left blank, which it is by default.
- The only surface that shows a name's transcripts is the *bookmarks*
  panel inside that session's ⋯ menu. The Mesh list, where the
  transcripts are listed, knows nothing about names.
- Three vocabularies describe one transcript: the *name* (`@dw-optimize`,
  in the rail and the session bar), the *title* (the first user message,
  in the Mesh list: `<command-message>fabric:init</command-message>…`),
  and the *bookmark label* (in the ⋯ panel). None of the three surfaces
  shows another's word.

## 2. What actually happened

**The branch.** `/branch` with nothing after it is carry. The node
bookmarked the tip under the agent's title, stopped the live process,
and relaunched it as a fork of the head. `@dw-optimize` now runs on the
fork; the original transcript is a bookmark of `@dw-optimize` named
after the session's title. That is the design working as written, and
it is the opposite of what the operator meant: they wanted a new
transcript and for the original to stay as it was. The console said
"branched — the previous tip is bookmarked", which is true and did not
say which transcript the name now runs on.

**The Mesh list.** `GET /api/sessions?repo=` enumerates transcript
files: id, first-message title, entrypoint, message count. No agent
name, no current/earlier role, no lineage. The row's label is the title;
the only Aspen mark is the `aspen` entrypoint chip. So the two rows in
the screenshot are `@dw-optimize`'s current transcript and the one it
left, and nothing on the page can say so. The row's one verb, *resume*,
mints a **new** name (defaulting to a slug of the title) on the
transcript, even when a name already runs on it. Two names on one
transcript is a state nothing else in Aspen expects.

**The ⎇ failure last week** (reconstructed; the node is another
machine): `branch_agent` writes the bookmark *before* the fork, then
`fork_to` stops the live process and spawns the fork. A fork has no
transcript file of its own until its first turn; until then the agent
row still points at the parent (`set_fork_pending`). If the spawn fails,
or succeeds and nothing is sent to it, the picture the operator sees is
exactly "the old session continued as it was, the new one is nowhere"
plus a stray bookmark of the tip. A bookmark written before the thing
it makes room for exists is the defect; the missing "nowhere" is a fork
that has not been spoken to yet, which the console does not say.

**The TUI branch.** Claude's own `/branch` from the terminal is seen by
adoption and raised as a need in the bell, off the page where the
transcript now sits. Nothing on the Mesh row says "branch of
`@dw-optimize`, no name."

## 3. Why the model fights the operator

1. **The verb answers a question the operator was not asked.** "Branch"
   in every other tool they use (git, Claude's TUI) makes a new thing
   and leaves the old one alone. Carry, which moves the name, is the
   surprising choice, and it is the silent default.
2. **Identity is managed in the wrong room.** What a name points at is
   shown and changed only from inside that name's session. The place an
   operator goes to see transcripts, the Mesh list, cannot show or
   change it.
3. **Three words for one thing.** A transcript is a title on one page, a
   name on another, a bookmark label on a third. The operator cannot
   carry a fact from one page to the next.
4. **A name can be anything but re-pointed.** Spawn, revive, move to a
   node, split, carry onto a fork, resume a bookmark: every verb exists
   except "put `@x` on that transcript."

## 4. The change

### 4.1 Two facts per transcript, in the operator's words

Keep **name**; keep **transcript** (the operator's own word; no coinage).
Per transcript the console shows two facts and, when known, its origin:

- **which name is on it**, one or none;
- **whether it is that name's current transcript or one it left**;
- **where it came from**: "branch of `@x`", when adoption or lineage
  knows.

So a row's first line is one of:

```
@dw-optimize · current
@dw-optimize · earlier · "Lakehouse shortcut dependency mapping"
branch of @dw-optimize · no name
no name
```

The store keeps its head / bookmark / lineage tables unchanged; the UI
never says "head", "bookmark", "fork" or "unclaimed". One verb word in
the UI: **branch**, never "fork".

### 4.2 The branch verb

**Decided 2026-09-21 (§8):** move (carry) stays the default. Bare
`/branch` means "a branch for this session, keeping its identity";
`/branch <name>` means "a new session with a new identity, in parallel."
What changes is that **nothing acts without the card being seen**. The
console already intercepts `/branch` (it is an Aspen-level command,
never sent to the harness), so the card costs no protocol work. The ⎇
button, the ⋯ menu row, `/branch`, `/fork` and `/branch <name>` all open
one card:

```
Branch @dw-optimize here
  ● Move @dw-optimize onto the branch
    This transcript stays as an earlier point of @dw-optimize.  label [optional]
    Restarts @dw-optimize; a running turn is interrupted.
  ○ Start a new agent on the branch            @ [dw-optimize-2]
    @dw-optimize keeps this transcript and keeps running.
                                        [Move @dw-optimize]   [Cancel]
```

- Bare `/branch` or `/fork`: the card, move preselected, the label
  field focused; Enter confirms.
- `/branch <name>`: the card, "start a new agent" preselected with the
  name filled; Enter confirms. (Today that argument is the bookmark
  label, which is the mismatch that bit the operator; the label is a
  field on the card instead.) `/branch <name> now` skips the card, for
  the keyboard.
- The button says the outcome (`Move @dw-optimize` / `Start
  @dw-optimize-2`). A taken name is rejected inline before submit.
- The confirmation names both: "`@dw-optimize` is now on the branch;
  this point is kept as an earlier transcript (label …). [open it]" or
  "Started `@dw-optimize-2` on a branch of this transcript.
  `@dw-optimize` is unchanged. [open `@dw-optimize-2`]". A split still
  navigates to the new session, as today.
- **Undo.** Until the fork's first turn nothing exists to lose: the
  confirmation offers *undo*, which stops the fork and puts the name
  back where it was without a re-fork. After a turn, the honest path is
  "move `@x` back to the earlier transcript", and the card says that is
  a branch.
- **Atomic.** The bookmark is written after the fork is spawned; on
  failure the name is left where it was, running, `fork_pending` is
  cleared, and the message says "Couldn't start the branch.
  `@dw-optimize` is unchanged and still running."
- **No turn yet.** A fork that has not taken its first turn has no file,
  so the Mesh list cannot show it; the *name* carries it: rail, Now and
  the session bar show "branched from `@dw-optimize` · no turn yet —
  send something to start the transcript." Stopping such a fork loses
  nothing, and the stop confirmation says so.
- Claude's own `/branch` typed in a terminal TUI cannot be intercepted
  (no protocol hook); adoption notices the file afterwards and §4.3
  puts its verbs on the row.

### 4.3 The Mesh list shows names, grouped by name

`GET /api/sessions?repo=` gains, per transcript, `agent`, `state`
(`current` | `earlier`), `label`, and `branch_of` (name, when known).
The list is grouped by name; a group's current transcript leads, its
earlier ones indent under it; transcripts with no name collapse into
one row at the end, which is where adoption's "ignore" accumulates:

```
@dw-optimize   current · running          claude · 156 msgs · 1h        [open]  ⋯
   fabric:init …
   earlier · "Lakehouse shortcut dependency mapping"     1 msg · 5h   [resume…]  ⋯
branch of @dw-optimize · no name             12 msgs · 2h              [resume…]  ⋯
no name · 27 ▸                                                          (collapsed)
```

- **One primary verb per row; the rest in ⋯.** Current + running:
  *open*. Current + down: *revive*. Earlier, branch-of, no name:
  *resume…*, which opens a two-line chooser of the branch card's shape:
  "as a new agent `@[ ]`" / "move `@dw-optimize` here (restarts it)".
  The ⋯ holds *ignore* (branch-of rows), *forget this earlier point*,
  *delete transcript* where it exists today.
- The title is the second line, with `<command-message>` and similar
  tags stripped (a bug on its own). An operator-set agent title, when
  set, is the second line instead. The `mcc` register's name folds into
  that same line rather than a third vocabulary.
- The filter matches names, labels, titles and session-id prefixes (the
  TUI shows ids; that is how "which name is on this transcript" is
  answered from the other direction).
- Phone: two lines per row, `@name · state` then `meta · [one verb in
  words] · ⋯`; groups collapsed except those holding a current
  transcript; the chooser is a sheet, never an inline input.

### 4.4 Move a name onto a transcript

One node verb, `POST /agents/{name}/move-to {session, at?}` (the API name
is open; the UI word is **move**): the current transcript, if different,
is kept as an earlier point with reason `move`; the name relaunches on a
branch of the target (never in place, for the reason resume-elsewhere
branches: one writer per transcript). Live names restart, as carry does
today, and the card says so. The Mesh row's chooser, the ⋯ panel, the
adoption card ("move `@x` here" replaces "carry `@x` here") and the
palette ("move `@dw-optimize` onto…") all call it. Resume-bookmark
without `as` is the same operation and folds into it at the API; the
row keeps *resume…* as its word because that is the operator's intent.

### 4.5 The ⋯ panel: "transcripts", not "bookmarks"

The session's bookmarks panel becomes the name's *transcripts*: the
current one first (with its title, so the Mesh list's word appears here
too), then earlier ones, then branches of any of them with no name, in
the Mesh row's format with the Mesh row's verbs. One component, both
places. Lineage in words: "branched from `@x` at message 40 · 2h", not
id prefixes.

### 4.6 One transcript, one name

Spawning or resuming onto a transcript that is already some name's
current one is refused with the name ("that is `@dw-optimize`'s current
transcript — open it, or branch it"). The Mesh row never offers the verb
(§4.3); this is the API's guard for the CLI and old consoles. Existing
double-named transcripts (made by Mesh *resume*) list as
`@a, @b · current` with a ⋯ verb to resolve.

### 4.7 Names have a life of their own

- **Rename** a name (`POST /agents/{name}/rename`): the bus address,
  rail entry, board slots and history follow; the running process is
  untouched (the name is Aspen's, not the harness's). Needed once
  `-2` names exist.
- **Delete** a name (exists): the card says its transcripts stay listed
  as *no name*.

## 5. Slate

| id | ask | tier |
|---|---|---|
| S-1 | Branch card on every entry (⎇, menu, `/branch`, `/fork`, `/branch <name>`): move preselected bare, new-name preselected with a name; prefilled selected name; button says the outcome; confirmation names both; undo before the first turn. | 1 |
| S-2 | Branch atomic: bookmark after the spawn; failure leaves the name running where it was; "no turn yet" on rail/Now/bar for a fresh branch. | 1 |
| S-3 | `agent`/`state`/`label`/`branch_of` on `GET /api/sessions`; Mesh list grouped by name, one primary verb + ⋯, title second line with tags stripped, filter matches names and ids; phone rows. | 1 |
| S-4 | Move a name onto a transcript: one node verb; bookmark-resume folds into it; adoption card and palette use the word. | 1 |
| S-5 | The ⋯ "bookmarks" panel becomes "transcripts", sharing the Mesh row component; lineage in words. | 2 |
| S-6 | Adoption verbs on the Mesh row for branch-of rows (bell card stays). | 2 |
| S-7 | Refuse a second name on a current transcript; list and resolve existing doubles. | 1 |
| S-8 | Rename a name. | 2 |

## 6. Questions

1. **Default of branch.** Answered: move (carry) stays the default;
   the card always shows first (§4.2, §8).
2. **The prefilled name.** `<name>-2`, selected, Enter accepts. Or a
   required blank field?
3. **The word.** *move* for putting a name on a transcript ("move `@x`
   to plank" / "move `@x` here" already read alike), replacing "carry"
   on the adoption card. Agree?
4. **State words.** current · earlier · branch of `@x` · no name. Or
   keep bookmark/head as words the operator sees?

## 7. UX review (2026-09-21)

Folded in above. The reviewer's points, in short: name + transcript,
not "line"; two facts per transcript (which name, current or earlier),
lineage shown beside them, not as a fourth role; the branch button must
say what will happen; prefilled selected name, never a required blank
(hostile on a phone and to the keyboard); one primary verb per row and
group by name, since 30 transcripts in a repo is normal; *move* is the
word the operator already has; undo is free before the first turn;
rename is missing; strip the command tags from titles regardless.

## 8. Decisions

1. **2026-09-21, the operator:** the 2026-09-04 model stands. Bare
   `/branch` is a branch for this session keeping its identity (move);
   `/branch <name>` is a new identity running in parallel (split). The
   change is the card: every entry to the verb shows it, preselected to
   the reading of the command, and nothing acts unseen. (The operator's
   own report: they did what was agreed but expected a prompt.)
2. **2026-09-21, the operator ("go for it"):** the reviewer's answers
   stand for questions 2–4 — the prefilled selected `<name>-N`; *move* as
   the one word (the adoption card's "carry" becomes "move @x here");
   the state words current · earlier · branch of `@x` · no name.
3. Built as v0.36: S-1..S-4, S-7. Rig notes: the live-elsewhere gate
   fires on the transcript the node itself just stopped writing, so undo
   and the failed-fork revive answer it with `in_place`; a pending fork
   still points at its parent's transcript and must not count as a name
   on it (the listing, the one-name guard).
