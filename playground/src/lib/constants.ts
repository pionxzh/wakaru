export const LEVELS = [
  { value: "minimal", label: "Minimal" },
  { value: "standard", label: "Standard" },
  { value: "aggressive", label: "Aggressive" },
] as const;

export type Level = (typeof LEVELS)[number]["value"];

export const FORMATTERS = [
  { value: "oxc", label: "Oxc" },
  { value: "none", label: "None" },
] as const;

export type Formatter = (typeof FORMATTERS)[number]["value"];
