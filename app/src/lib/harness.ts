// What the Harnesses screen says about one harness, kept apart from the screen so it can be tested.
import type { Harness } from "./api";

export type Tone = "ok" | "warn" | "off";

/** The one-line state shown at the right of a harness's card. */
export function harnessState(h: Harness): { tone: Tone; text: string } {
  if (!h.installed) return { tone: "off", text: "Not on this computer" };
  switch (h.link.state) {
    case "connected":
      return { tone: "ok", text: "Uses 8gent" };
    case "not_connected":
      return { tone: "warn", text: "Not pointed at 8gent" };
    case "unreadable":
      return { tone: "warn", text: "Its settings were not read" };
    default:
      return { tone: "off", text: h.connect_kind === "none" ? "Cannot use 8gent yet" : "Set from its own side" };
  }
}

/** Whether 8gent's own entry is among the ones that point here (the only one it will remove). */
export function hasOwnEntry(h: Harness): boolean {
  return h.link.state === "connected" && h.link.entries.includes("8gent");
}

/** "uzu, tensorfold and 8gent" */
export function listed(names: readonly string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** How the harness reaches 8gent, in words, when it does. */
export function linkedThrough(h: Harness): string {
  if (h.link.state !== "connected") return "";
  const n = h.link.entries.length;
  return `Through ${n === 1 ? "the provider" : "the providers"} ${listed(h.link.entries)} in its settings.`;
}
