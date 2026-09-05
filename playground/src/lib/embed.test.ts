import { describe, expect, it } from "vitest";
import { readEmbedFlag, standaloneUrl } from "./embed";

describe("embed mode", () => {
  it("is on only for embed=1", () => {
    expect(readEmbedFlag("?embed=1")).toBe(true);
    expect(readEmbedFlag("?embed=true")).toBe(false);
    expect(readEmbedFlag("")).toBe(false);
  });

  it("strips the embed flag and keeps the share hash", () => {
    expect(standaloneUrl("https://wakarujs.com/playground/?embed=1#state=1|abc")).toBe(
      "https://wakarujs.com/playground/#state=1|abc"
    );
  });
});
