// ESM sources for the CommonJS export-storage matrix. Every case is compiled
// to CommonJS by each producer, then decompiled back to ESM. `driver` runs
// against the module namespace `m` (live bindings for ESM, the exports object
// for CommonJS) and records observations with `log`; the original ESM, the
// producer's CommonJS, and the recovered ESM must record the same log.
//
// Cases target how a producer stores an exported value: in the `exports`
// property itself, in a local binding mirrored into the property on every
// write, or in a local binding exposed through a getter. Writes inside
// functions, before the declaration, and through patterns are the shapes that
// distinguish those encodings.
export const cases = {
  "counter-mutated-in-function": {
    files: {
      "mod.js": `
export let count = 0;
export const limit = 3;
export function bump() { count += 1; return count < limit; }
export function reset() { count = 0; }
`,
    },
    driver: `log(m.count); log(m.bump()); log(m.count); m.bump(); m.bump(); log(m.bump()); log(m.count); m.reset(); log(m.count);`,
  },
  "toplevel-reassign": {
    files: {
      "mod.js": `
export let x = 1;
x = 2;
x += 3;
x++;
export function get() { return x; }
`,
    },
    driver: `log(m.x); log(m.get());`,
  },
  "declared-then-assigned": {
    files: {
      "mod.js": `
export let y;
export var z;
function compute() { return 42; }
y = compute();
z = y + 1;
export function setY(v) { y = v; z = v + 1; }
`,
    },
    driver: `log(m.y); log(m.z); m.setY(7); log(m.y); log(m.z);`,
  },
  "update-forms": {
    files: {
      "mod.js": `
export let n = 0;
export let s = "";
export function step() { n++; ++n; n--; n **= 2; s += "a"; n ||= 9; return n; }
export function loop() { for (n = 0; n < 3; n++) s += n; return s; }
`,
    },
    driver: `log(m.step()); log(m.n); log(m.loop()); log(m.n); log(m.s);`,
  },
  "destructuring-writes": {
    files: {
      "mod.js": `
export let a = 1, b = 2;
export function swap() { [a, b] = [b, a]; }
export function set(o) { ({ a, b = 5 } = o); }
export function each(xs) { for (a of xs); return a; }
`,
    },
    driver: `m.swap(); log([m.a, m.b]); m.set({ a: 9 }); log([m.a, m.b]); log(m.each([1, 2, 3])); log(m.a);`,
  },
  "destructuring-export-decl": {
    files: {
      "mod.js": `
const src = { p: 1, q: 2, r: [3, 4] };
export const { p, q: qq } = src;
export let [first, second] = src.r;
export function bumpFirst() { first += 10; return first; }
`,
    },
    driver: `log([m.p, m.qq, m.first, m.second]); log(m.bumpFirst()); log(m.first);`,
  },
  "alias-export-mutated": {
    files: {
      "mod.js": `
let internal = 1;
export { internal as value, internal as other };
export function inc() { internal++; return internal; }
`,
    },
    driver: `log([m.value, m.other]); log(m.inc()); log([m.value, m.other]);`,
  },
  "const-read-in-function": {
    files: {
      "mod.js": `
export const config = { mode: "a" };
export const LIMIT = 10;
export function describe() { return config.mode + LIMIT; }
export function mutate() { config.mode = "b"; return describe(); }
`,
    },
    driver: `log(m.describe()); log(m.mutate()); log(m.config.mode);`,
  },
  "functions-call-each-other": {
    files: {
      "mod.js": `
export function isEven(n) { return n === 0 ? true : isOdd(n - 1); }
export function isOdd(n) { return n === 0 ? false : isEven(n - 1); }
export const twice = (f, v) => f(f(v));
export function useTwice() { return twice((v) => v + 1, 0); }
`,
    },
    driver: `log(m.isEven(4)); log(m.isOdd(3)); log(m.useTwice());`,
  },
  "function-reassigned": {
    files: {
      "mod.js": `
export function impl() { return "slow"; }
export function upgrade() { impl = function () { return "fast"; }; }
export function callImpl() { return impl(); }
`,
    },
    driver: `log(m.callImpl()); m.upgrade(); log(m.callImpl()); log(m.impl());`,
  },
  "class-export": {
    files: {
      "mod.js": `
export class Point {
  constructor(x) { this.x = x; }
  static origin() { return new Point(0); }
  clone() { return new Point(this.x); }
}
export function make(x) { return new Point(x); }
export class Sub extends Point { twice() { return this.x * 2; } }
`,
    },
    driver: `log(m.Point.origin().x); log(m.make(3).clone().x); log(new m.Sub(4).twice()); log(m.make(1) instanceof m.Point);`,
  },
  "default-and-named": {
    files: {
      "mod.js": `
export let hits = 0;
export default function main() { hits++; return helper(); }
export function helper() { return "h" + hits; }
`,
    },
    driver: `log(m.default()); log(m.hits); log(m.helper());`,
  },
  "default-expression-mutated": {
    files: {
      "mod.js": `
let state = { v: 1 };
export default state;
export function replace() { state = { v: 2 }; return state.v; }
`,
    },
    driver: `log(m.default.v); log(m.replace()); log(m.default.v);`,
  },
  "default-let-binding": {
    files: {
      "mod.js": `
let current = "a";
export { current as default };
export function change() { current = "b"; }
`,
    },
    driver: `log(m.default); m.change(); log(m.default);`,
  },
  "async-and-generator": {
    files: {
      "mod.js": `
export let ticks = 0;
export async function tick() { await null; ticks++; return ticks; }
export function* gen() { while (ticks < 3) yield ticks++; }
`,
    },
    driver: `log(await m.tick()); log([...m.gen()]); log(m.ticks);`,
  },
  "closure-arrow-writes": {
    files: {
      "mod.js": `
export let total = 0;
export const add = (v) => (total += v);
export const handlers = [() => total++, () => { total = -1; }];
`,
    },
    driver: `log(m.add(5)); m.handlers[0](); log(m.total); m.handlers[1](); log(m.total);`,
  },
  "shadowed-name": {
    files: {
      "mod.js": `
export let count = 0;
export function setCount(count) { return count; }
export function setOuter(value) { count = value; { let count = 99; } return count; }
`,
    },
    driver: `log(m.setCount(5)); log(m.count); log(m.setOuter(3)); log(m.count);`,
  },
  "reexport-named": {
    files: {
      "dep.js": `
export let depCount = 0;
export function depInc() { depCount++; }
export default "depDefault";
`,
      "mod.js": `
export { depCount, depInc, default as depDefault } from "./dep.js";
export { depCount as renamed } from "./dep.js";
`,
    },
    driver: `log(m.depCount); m.depInc(); log(m.depCount); log(m.renamed); log(m.depDefault);`,
  },
  "reexport-star": {
    files: {
      "dep.js": `
export let starCount = 0;
export function starInc() { starCount++; }
`,
      "mod.js": `
export * from "./dep.js";
export * as ns from "./dep.js";
export const own = 1;
`,
    },
    driver: `log(m.starCount); m.starInc(); log(m.starCount); log(m.ns.starCount); log(m.own);`,
  },
  "import-then-export": {
    files: {
      "dep.js": `
export let live = 0;
export function bumpLive() { live++; }
`,
      "mod.js": `
import { live, bumpLive } from "./dep.js";
export { live, bumpLive };
export function readLive() { return live; }
`,
    },
    driver: `log(m.live); m.bumpLive(); log(m.live); log(m.readLive());`,
  },
  "dynamic-import": {
    files: {
      "dep.js": `
export let live = 0;
export function bumpLive() { live++; }
export default function depFn() { return "fn"; }
`,
      "mod.js": `
export async function loadNs() { return await import("./dep.js"); }
export function loadThen() { return import("./dep.js").then((ns) => ns.live); }
export async function loadDefault() { const { default: f } = await import("./dep.js"); return f(); }
export function loadByName(name) { return import("./" + name + ".js").then((ns) => ns.live); }
`,
    },
    driver: `const ns = await m.loadNs(); log(ns.live); ns.bumpLive(); log(ns.live); log(await m.loadThen()); log(await m.loadDefault()); log(await m.loadByName("dep"));`,
  },
  "imported-used-in-function": {
    files: {
      "dep.js": `
export let depValue = 1;
export function setDep(v) { depValue = v; }
export default function depFn() { return "fn"; }
`,
      "mod.js": `
import depFn, { depValue, setDep } from "./dep.js";
import * as all from "./dep.js";
export function read() { return [depValue, all.depValue, depFn()]; }
export function write(v) { setDep(v); return read(); }
`,
    },
    driver: `log(m.read()); log(m.write(5));`,
  },
  "hoisted-call-before-init": {
    files: {
      "mod.js": `
export var seen = [];
record("early");
export function record(v) { seen.push(v); return seen.length; }
record("late");
`,
    },
    driver: `log(m.seen); log(m.record("x"));`,
  },
  "name-collisions": {
    files: {
      "mod.js": `
const exportsLike = 1;
export let module = "m";
export let require = "r";
export function f() { module += "!"; return module + require + exportsLike; }
`,
    },
    driver: `log(m.f()); log(m.module); log(m.require);`,
  },
  "string-export-name": {
    files: {
      "mod.js": `
let v = 1;
export { v as "a-b" };
export function bump() { v++; }
`,
    },
    driver: `log(m["a-b"]); m.bump(); log(m["a-b"]);`,
  },
  "conditional-assignment": {
    files: {
      "mod.js": `
export let mode = "init";
export function configure(flag) { if (flag) mode = "on"; else mode = "off"; return mode; }
export const toggle = () => (mode = mode === "on" ? "off" : "on");
`,
    },
    driver: `log(m.configure(true)); log(m.toggle()); log(m.mode);`,
  },
  "object-method-writes": {
    files: {
      "mod.js": `
export let level = 0;
export const api = {
  up() { level++; return level; },
  get current() { return level; },
  set current(v) { level = v; },
};
`,
    },
    driver: `log(m.api.up()); m.api.current = 5; log(m.level); log(m.api.current);`,
  },
  "class-method-writes": {
    files: {
      "mod.js": `
export let created = 0;
export class Widget {
  static count() { return created; }
  constructor() { created++; }
  #secret = created;
  reveal() { return this.#secret; }
}
`,
    },
    driver: `new m.Widget(); const w = new m.Widget(); log(m.created); log(m.Widget.count()); log(w.reveal());`,
  },
  "typeof-guarded-init": {
    files: {
      "mod.js": `
export let maybe;
export function init() { if (typeof maybe === "undefined") maybe = 1; return maybe; }
`,
    },
    driver: `log(typeof m.maybe); log(m.init()); log(m.maybe);`,
  },
};
