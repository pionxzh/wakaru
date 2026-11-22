import { tag } from "./shared.cjs";
import { mark } from "./marker.js";

export default function run() {
  return mark(tag("lazy"));
}
