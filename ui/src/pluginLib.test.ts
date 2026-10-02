import { describe, expect, it } from "vitest";
import type { CatalogPlugin, PluginRegistry } from "./api";
import { libraryIndex, providesParts, searchPlugins, whereItLives } from "./pluginLib";

const p = (name: string, extra: Partial<CatalogPlugin> = {}): CatalogPlugin => ({ marketplace: "m", name, current: "1", source: "./x", cached: [], ...extra });

describe("plugin library helpers", () => {
  it("ranks a name prefix above a description hit and needs every word", () => {
    const cat = [p("aaa", { description: "talks to github" }), p("github"), p("gitlab", { author: "GitLab" })];
    expect(searchPlugins(cat, "git").map((x) => x.name)).toEqual(["github", "gitlab", "aaa"]);
    expect(searchPlugins(cat, "git lab").map((x) => x.name)).toEqual(["gitlab"]);
    expect(searchPlugins(cat, "")).toEqual([]);
  });

  it("reads the node's members, or derives them from an older node", () => {
    const reg = (extra: Partial<PluginRegistry>): PluginRegistry => ({
      marketplaces: [],
      rules: [{ id: "r", marketplace: "big", plugin: "b3", scope_kind: "session", scope: "x", enabled: false, updated_at: 1 }],
      catalog: { synced_at: {}, errors: {}, plugins: [...Array.from({ length: 30 }, (_, i) => p(`b${i}`, { marketplace: "big" })), p("s1", { marketplace: "small" })] },
      node: null,
      ...extra,
    });
    const older = libraryIndex(reg({}));
    expect([...older.entries()].sort()).toEqual([["b3@big", "rule"], ["s1@small", "marketplace"]]);
    const newer = libraryIndex(reg({ library_members: [{ marketplace: "big", plugin: "b7", why: "picked" }] }));
    expect([...newer.keys()]).toEqual(["b7@big"]);
  });

  it("says what a plugin brings and where it lives", () => {
    expect(providesParts({ skills: 1, commands: 2, agents: 0, hooks: true, mcp: 1, lsp: 0 })).toEqual(["1 skill", "2 commands", "1 MCP server", "hooks"]);
    expect(whereItLives(p("x"))).toBe("in the marketplace's repo");
    expect(whereItLives(p("x", { source: { source: "git-subdir", url: "https://github.com/adobe/skills.git", path: "plugins/c" } }))).toBe("github.com/adobe/skills · plugins/c");
  });
});
