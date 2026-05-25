import greet, { named } from "./dep.js";

export const value = named + 1;

export async function run() {
  const mod = await import("./lazy.js");
  return greet(value + mod.extra + import.meta.url.length);
}
