import { describe, expect, it } from "vitest";
import { DEFAULT_EXAMPLE, EXAMPLES } from "./examples";

describe("playground examples", () => {
  it("keeps the Babel example as the default source", () => {
    expect(DEFAULT_EXAMPLE).toBe(EXAMPLES[0].source);
    expect(EXAMPLES[0].value).toBe("babel");
  });

  it("has unique values and labels", () => {
    const values = EXAMPLES.map(({ value }) => value);
    const labels = EXAMPLES.map(({ label }) => label);
    expect(new Set(values).size).toBe(EXAMPLES.length);
    expect(new Set(labels).size).toBe(EXAMPLES.length);
  });

  it("has non-empty, newline-terminated sources", () => {
    for (const { source } of EXAMPLES) {
      expect(source.trim().length).toBeGreaterThan(0);
      expect(source.endsWith("\n")).toBe(true);
    }
  });

  it("parses every example as a script", () => {
    for (const { value, source } of EXAMPLES) {
      // Function() throws a SyntaxError on invalid JS without executing it.
      expect(() => new Function(source), value).not.toThrow();
    }
  });
});
