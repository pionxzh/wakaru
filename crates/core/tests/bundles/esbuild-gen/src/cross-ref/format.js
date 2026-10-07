import { add, multiply, PI } from "./math.js";

export function formatSum(a, b) { return a + " + " + b + " = " + add(a, b); }
export function formatProduct(a, b) { return a + " * " + b + " = " + multiply(a, b) + " (PI=" + PI + ")"; }
