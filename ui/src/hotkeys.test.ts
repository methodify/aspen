import { describe, expect, it } from "vitest";
import { chordMatches } from "./hotkeys";

const ev = (key: string, code: string, m: Partial<{ altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }> = {}) => ({
  key, code, altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, ...m,
});

describe("chordMatches", () => {
  it("matches plain keys only without modifiers", () => {
    expect(chordMatches("i", ev("i", "KeyI"))).toBe(true);
    expect(chordMatches("i", ev("i", "KeyI", { altKey: true }))).toBe(false);
  });
  it("matches alt chords by the physical key", () => {
    expect(chordMatches("alt+1", ev("¡", "Digit1", { altKey: true }))).toBe(true);
    expect(chordMatches("alt+1", ev("1", "Digit1"))).toBe(false);
    expect(chordMatches("alt+ArrowLeft", ev("ArrowLeft", "ArrowLeft", { altKey: true }))).toBe(true);
    expect(chordMatches("alt+z", ev("Ω", "KeyZ", { altKey: true }))).toBe(true);
    expect(chordMatches("alt+.", ev("≥", "Period", { altKey: true }))).toBe(true);
  });
  it("tells shift chords apart", () => {
    expect(chordMatches("alt+shift+[", ev("{", "BracketLeft", { altKey: true, shiftKey: true }))).toBe(true);
    expect(chordMatches("alt+[", ev("{", "BracketLeft", { altKey: true, shiftKey: true }))).toBe(false);
    expect(chordMatches("alt+[", ev("[", "BracketLeft", { altKey: true }))).toBe(true);
  });
});
