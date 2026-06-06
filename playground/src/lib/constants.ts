export const LEVELS = [
  { value: "minimal", label: "Minimal" },
  { value: "standard", label: "Standard" },
  { value: "aggressive", label: "Aggressive" },
] as const;

export type Level = (typeof LEVELS)[number]["value"];
