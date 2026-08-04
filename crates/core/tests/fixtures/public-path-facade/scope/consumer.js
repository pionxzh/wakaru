import readDefault, { liveValue, namedValue } from "./index-hash.js";
import * as namespace from "./index-hash.js";
import "./index-hash.js";
export { namedValue as forwarded } from "./index-hash.js";
export * from "./index-hash.js";
const literalLoad = import("./index-hash.js");
const publicPath = "./index-hash.js";
const computedLoad = import(publicPath);
console.log(readDefault(), liveValue, namedValue, namespace, literalLoad, computedLoad);
