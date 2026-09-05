import { describe, expect, it } from "vitest";
import { createShareUrl, readShareState, type PlaygroundShareState } from "./share";

const sharedState: PlaygroundShareState = {
  source: "const value = 1;",
  mode: "roundtrip",
  producer: "swc",
  level: "minimal",
  formatter: false,
  mapping: true,
  vueSfc: true,
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

  it("defaults the mapping view off for older shared links", () => {
    const legacyState = { ...sharedState } as Partial<PlaygroundShareState>;
    delete legacyState.mapping;
    const url = createShareUrl(
      legacyState as PlaygroundShareState,
      "https://wakaru.vercel.app/playground/"
    );

    expect(readShareState(new URL(url).hash)).toEqual({
      ...legacyState,
      mapping: false,
    });
  });

  it("defaults Vue SFC recovery off for older shared links", () => {
    const legacyState = { ...sharedState } as Partial<PlaygroundShareState>;
    delete legacyState.vueSfc;
    const url = createShareUrl(
      legacyState as PlaygroundShareState,
      "https://wakaru.vercel.app/playground/"
    );

    expect(readShareState(new URL(url).hash)).toEqual({
      ...legacyState,
      vueSfc: false,
    });
  });

  it("defaults older shared links to decompile mode with Babel selected", () => {
    const legacyState = { ...sharedState } as Partial<PlaygroundShareState>;
    delete legacyState.mode;
    delete legacyState.producer;
    const url = createShareUrl(
      legacyState as PlaygroundShareState,
      "https://wakaru.vercel.app/playground/"
    );

    expect(readShareState(new URL(url).hash)).toEqual({
      ...legacyState,
      mode: "decompile",
      producer: "babel",
    });
  });
});
