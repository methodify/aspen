import { describe, expect, it } from "vitest";
import { codePathTarget } from "./pathLinks";

describe("codePathTarget", () => {
  it("links code spans that are whole paths", () => {
    expect(codePathTarget("docs/report.md")).toBe("docs/report.md");
    expect(codePathTarget("report.md")).toBe("report.md");
    expect(codePathTarget("/tmp/out.csv")).toBe("/tmp/out.csv");
    expect(codePathTarget("~/notes/plan.md")).toBe("~/notes/plan.md");
    expect(codePathTarget("C:\\work\\a.txt")).toBe("C:\\work\\a.txt");
    expect(codePathTarget("file:///home/u/x.pdf")).toBe("/home/u/x.pdf");
  });
  it("leaves code that is not a path alone", () => {
    for (const s of ["cargo build", "foo.bar()", "x = 1", "npm run build", "README", "a/b", "/", "--flag", "v0.49.5"]) {
      expect(codePathTarget(s), s).toBeNull();
    }
  });
});
