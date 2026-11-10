import { describe, expect, it } from "vitest";
import { memoryAt, seconds, windowFor, windowLabel, windowsFor } from "./format";

describe("context windows", () => {
  it("are named in 1024s", () => {
    expect(windowLabel(65536)).toBe("64K");
    expect(windowLabel(1048576)).toBe("1M");
    expect(windowLabel(100000)).toBe("100,000");
  });

  it("are offered up to the model's maximum, with the current one always there", () => {
    expect(windowsFor(262144, 65536)).toEqual([16384, 32768, 65536, 131072, 262144]);
    expect(windowsFor(32768, 32768)).toEqual([16384, 32768]);
    expect(windowsFor(1048576, 65536)).toContain(1048576);
    expect(windowsFor(262144, 100000)).toContain(100000);
    // a model whose maximum is unknown gets the usual sizes, not the very large ones
    expect(windowsFor(null, null)).toEqual([16384, 32768, 65536, 131072, 262144]);
  });

  it("the window offered after a prompt did not fit has a quarter to spare", () => {
    expect(windowFor(94604, 262144)).toBe(131072);
    expect(windowFor(20015, 262144)).toBe(32768);
    expect(windowFor(60000, 65536)).toBeNull(); // nothing large enough that the model allows
  });
});

describe("memory", () => {
  const GB = 1e9;
  it("is weights and runtime plus the cache for the window, per conversation", () => {
    // uzu with a 9B model: 6.9 GB base, 57,344 bytes a token
    const one = memoryAt(6.9 * GB, 57344, 1, 65536, 19.07 * GB);
    expect(Math.round(one.bytes! / 1e8) / 10).toBe(10.7);
    expect(one.fit).toBe("fits");
    expect(memoryAt(6.9 * GB, 57344, 2, 65536, 19.07 * GB).fit).toBe("fits");
    expect(memoryAt(6.9 * GB, 57344, 1, 131072, 19.07 * GB).fit).toBe("fits");
    expect(memoryAt(6.9 * GB, 57344, 2, 131072, 19.07 * GB).fit).toBe("too-big");
    expect(memoryAt(6.9 * GB, 57344, 1, 180000, 19.07 * GB).fit).toBe("tight");
  });

  it("is unknown rather than guessed when a loader does not say", () => {
    expect(memoryAt(null, null, 1, 65536, 19 * GB)).toEqual({ bytes: null, fit: "unknown", share: 0 });
    expect(memoryAt(5 * GB, 32768, 1, 65536, null).fit).toBe("unknown");
  });
});

describe("durations", () => {
  it("read as milliseconds or seconds", () => {
    expect(seconds(340)).toBe("340 ms");
    expect(seconds(3200)).toBe("3.2 s");
    expect(seconds(null)).toBe("");
  });
});
