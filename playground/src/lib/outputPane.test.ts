import { describe, expect, it } from "vitest";
import { resolveOutputPaneView } from "./outputPane";

describe("resolveOutputPaneView", () => {
  it("falls back to javascript when nothing is requested", () => {
    expect(
      resolveOutputPaneView({
        diffRequested: false,
        diffAvailable: true,
        vueRequested: false,
        vueAvailable: true,
      })
    ).toBe("javascript");
  });

  it("shows the diff only while it is available", () => {
    const requested = { diffRequested: true, vueRequested: false, vueAvailable: false };
    expect(resolveOutputPaneView({ ...requested, diffAvailable: true })).toBe("diff");
    expect(resolveOutputPaneView({ ...requested, diffAvailable: false })).toBe("javascript");
  });

  it("shows the Vue SFC only once it is recovered", () => {
    const requested = { diffRequested: false, diffAvailable: false, vueRequested: true };
    expect(resolveOutputPaneView({ ...requested, vueAvailable: true })).toBe("vue");
    expect(resolveOutputPaneView({ ...requested, vueAvailable: false })).toBe("javascript");
  });

  it("prefers the diff over the Vue SFC when both are requested", () => {
    expect(
      resolveOutputPaneView({
        diffRequested: true,
        diffAvailable: true,
        vueRequested: true,
        vueAvailable: true,
      })
    ).toBe("diff");
  });
});
