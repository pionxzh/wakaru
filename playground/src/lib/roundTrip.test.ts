import { describe, expect, it } from "vitest";
import {
  getProducerDescriptor,
  isPlaygroundMode,
  isProducer,
  PRODUCERS,
  ROUND_TRIP_EXAMPLE,
} from "./roundTrip";

describe("round-trip playground configuration", () => {
  it("offers the three browser producers with a visible recipe", () => {
    expect(PRODUCERS.map(({ value }) => value)).toEqual([
      "babel",
      "swc",
      "esbuild",
    ]);
    expect(PRODUCERS.every(({ recipe }) => recipe.length > 0)).toBe(true);
  });

  it("validates shareable mode and producer values", () => {
    expect(isPlaygroundMode("roundtrip")).toBe(true);
    expect(isPlaygroundMode("compare")).toBe(false);
    expect(isProducer("swc")).toBe(true);
    expect(isProducer("typescript")).toBe(false);
  });

  it("uses Babel as a defensive descriptor fallback", () => {
    expect(getProducerDescriptor("babel").label).toBe("Babel");
  });

  it("ships an editable example that exercises JSX and modern syntax", () => {
    expect(ROUND_TRIP_EXAMPLE).toContain("user?.profile?.name");
    expect(ROUND_TRIP_EXAMPLE).toContain("<article");
    expect(ROUND_TRIP_EXAMPLE).toContain("??");
  });
});
