import { describe, expect, it } from "vitest";
import { createShareUrl, readShareState, type PlaygroundShareState } from "./share";

const sharedState: PlaygroundShareState = {
  source: "const value = 1;",
  level: "minimal",
  formatter: "none",
  version: "v1.4.0+test",
};

describe("playground share state", () => {
  it("round-trips level and formatter from a share URL hash", () => {
    const url = createShareUrl(sharedState, "https://wakaru.vercel.app/playground/");
    const hash = new URL(url).hash;

    expect(readShareState(hash)).toEqual(sharedState);
  });

  it("accepts percent-encoded hash separators from rendered links", () => {
    const url = createShareUrl(sharedState, "https://wakaru.vercel.app/playground/");
    const hash = new URL(url).hash.replace("|", "%7C");

    expect(readShareState(hash)).toEqual(sharedState);
  });
});
