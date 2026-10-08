import { describe, expect, it } from "vitest";
import { summarizeContext } from "./sessionExtras";

describe("summarizeContext", () => {
  it("reads a 1% session as 1%, not 100%", () => {
    expect(summarizeContext({ percentage: 1, totalTokens: 10000, maxTokens: 1000000 })?.percent).toBe(1);
    expect(summarizeContext({ percentage: 1 })?.percent).toBe(1);
  });
  it("prefers tokens when both are known", () => {
    expect(summarizeContext({ percentage: 4, totalTokens: 44131, maxTokens: 1000000 })?.percent).toBeCloseTo(4.4131);
  });
});
