import { label } from "./widget.js";
import * as shapes from "./shapes.js";
import { tag } from "./shared.cjs";
import { mark } from "./marker.js";

export let count = 0;
export function bump() {
  count += 1;
}
export function describe(value) {
  return mark(tag(label(value)));
}
export function kinds() {
  return Object.keys(shapes);
}
export function loadLazy() {
  return import("./lazy.js").then(({ default: run }) => run());
}
