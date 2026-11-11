import { describe, expect, it } from "vitest";
import type { Harness } from "./api";
import { harnessState, hasOwnEntry, linkedThrough, listed } from "./harness";

const base: Harness = {
  id: "hermes", name: "Hermes", summary: "", homepage: "", installed: true, program: "/x/hermes", version: "0.21.6",
  link: { state: "not_connected" }, can_connect: true, connect_kind: "settings", settings_file: "/x/config.yaml", use: "", runs_as_agent: true,
};

describe("harness state", () => {
  it("says not installed before anything else", () => {
    expect(harnessState({ ...base, installed: false, link: { state: "connected", entries: ["8gent"] } })).toEqual({ tone: "off", text: "Not on this computer" });
  });
  it("names each way an installed harness can stand", () => {
    expect(harnessState({ ...base, link: { state: "connected", entries: ["uzu"] } }).tone).toBe("ok");
    expect(harnessState(base)).toEqual({ tone: "warn", text: "Not pointed at 8gent" });
    expect(harnessState({ ...base, link: { state: "unreadable", why: "tabs" } }).text).toBe("Its settings were not read");
    expect(harnessState({ ...base, connect_kind: "own", link: { state: "not_by_eightgent", why: "x" } })).toEqual({ tone: "off", text: "Set from its own side" });
    expect(harnessState({ ...base, connect_kind: "none", link: { state: "not_by_eightgent", why: "x" } }).text).toBe("Cannot use 8gent yet");
  });
  it("only 8gent's own entry can be taken out again", () => {
    expect(hasOwnEntry({ ...base, link: { state: "connected", entries: ["uzu", "tensorfold"] } })).toBe(false);
    expect(hasOwnEntry({ ...base, link: { state: "connected", entries: ["uzu", "8gent"] } })).toBe(true);
    expect(hasOwnEntry(base)).toBe(false);
  });
  it("lists the entries in words", () => {
    expect(listed([])).toBe("");
    expect(listed(["8gent"])).toBe("8gent");
    expect(listed(["uzu", "8gent"])).toBe("uzu and 8gent");
    expect(listed(["uzu", "tensorfold", "8gent"])).toBe("uzu, tensorfold and 8gent");
    expect(linkedThrough({ ...base, link: { state: "connected", entries: ["8gent"] } })).toBe("Through the provider 8gent in its settings.");
    expect(linkedThrough({ ...base, link: { state: "connected", entries: ["uzu", "8gent"] } })).toBe("Through the providers uzu and 8gent in its settings.");
    expect(linkedThrough(base)).toBe("");
  });
});
