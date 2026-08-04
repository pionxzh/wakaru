import { total } from "./chunk-hash.js";
export { total as forwarded } from "./chunk-hash.js";
export * from "./chunk-hash.js";
const loaded = import("./chunk-hash.js");
console.log(total, loaded);
