import { describe, expect, it } from "vitest";
import { fmtAgo, fmtMessageTime, summarizeContext } from "./sessionExtras";

describe("summarizeContext", () => {
  it("reads a 1% session as 1%, not 100%", () => {
    expect(summarizeContext({ percentage: 1, totalTokens: 10000, maxTokens: 1000000 })?.percent).toBe(1);
    expect(summarizeContext({ percentage: 1 })?.percent).toBe(1);
  });
  it("prefers tokens when both are known", () => {
    expect(summarizeContext({ percentage: 4, totalTokens: 44131, maxTokens: 1000000 })?.percent).toBeCloseTo(4.4131);
  });
});

describe("message times", () => {
  const now = new Date(2026, 10, 23, 14, 30).getTime();
  it("say the day only when it is not today", () => {
    // Today: the time alone (in the reader's locale, 12- or 24-hour).
    const today = fmtMessageTime(new Date(2026, 10, 23, 9, 5).getTime(), now);
    expect(today).toMatch(/0?9.05/);
    expect(today.split(" ").length).toBeLessThanOrEqual(2);
    // This week: a weekday first.
    const week = fmtMessageTime(new Date(2026, 10, 21, 21, 4).getTime(), now);
    expect(week).toMatch(/^[^\d\s]+ /);
    expect(week).toMatch(/:04/);
    // Earlier: a date.
    expect(fmtMessageTime(new Date(2026, 9, 1, 8, 0).getTime(), now)).toMatch(/1/);
  });
  it("count back coarsely", () => {
    expect(fmtAgo(now - 40_000, now)).toBe("40s");
    expect(fmtAgo(now - 12 * 60_000, now)).toBe("12m");
    expect(fmtAgo(now - (2 * 60 + 14) * 60_000, now)).toBe("2h 14m");
    expect(fmtAgo(now - 27 * 3600_000, now)).toBe("1d 3h");
  });
});
