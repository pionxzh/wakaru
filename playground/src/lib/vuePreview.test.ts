import { describe, expect, it } from "vitest";
import { applyVuePreviewResult, resetVuePreview } from "./vuePreview";

describe("Vue preview state", () => {
  it("keeps the selected Vue view when a current recovery succeeds", () => {
    expect(applyVuePreviewResult({ sfc: "old", view: "vue" }, "new")).toEqual({
      sfc: "new",
      view: "vue",
    });
  });

  it("selects JavaScript when a current recovery has no Vue output", () => {
    expect(applyVuePreviewResult({ sfc: "old", view: "vue" }, null)).toEqual({
      sfc: null,
      view: "javascript",
    });
  });

  it("does not silently reselect Vue after the user is on JavaScript", () => {
    expect(applyVuePreviewResult({ sfc: null, view: "javascript" }, "recovered")).toEqual({
      sfc: "recovered",
      view: "javascript",
    });
  });

  it("selects the requested initial view when the feature is toggled", () => {
    expect(resetVuePreview(true)).toEqual({ sfc: null, view: "vue" });
    expect(resetVuePreview(false)).toEqual({ sfc: null, view: "javascript" });
  });
});
