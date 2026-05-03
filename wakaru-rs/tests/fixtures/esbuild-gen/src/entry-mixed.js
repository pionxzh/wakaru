import { clamp } from './utils-cjs.cjs';
import { padLeft } from './format-cjs.cjs';
import { isEmail } from './validate-cjs.cjs';
import { toUpper } from './convert-cjs.cjs';
import { unique } from './array-cjs.cjs';
import { keys } from './object-cjs.cjs';
export * as math from './math.js';
export * as greet from './greet.js';
export function main() {
  return padLeft(toUpper(isEmail("a@b") ? "yes" : "no"), 10) + clamp(42, 0, 100) + unique([1,1,2]).length + keys({a:1}).length;
}
