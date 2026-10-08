// NODE_PATH=<tslib node_modules> WAKARU=<binary> node --experimental-vm-modules runtime.cjs
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const vm = require("node:vm");
const { execFileSync } = require("node:child_process");
const tslib = require("tslib");
const binary = process.env.WAKARU;
assert(binary, "Set WAKARU to the binary under test");
const native = fs.readFileSync(path.join(__dirname, "source.ts"), "utf8");
const method = "Parent.prototype.value = function() { return this.base; };";
const ordinary = `function Parent(value) { this.base = value; } ${method}`;
const fields = '[41,"main","main",41]';
// The parent's object becomes `this` and skips Child.prototype.
const returned = '[41,"main","main",40]';
const cases = [
  { name: "ordinary", setup: ordinary, lowered: fields, standard: fields },
  { name: "returned object", setup: `function Parent(value) { return Object.create(Parent.prototype, { base: { value } }); } ${method}`, lowered: returned, standard: returned },
  { name: "native parent", setup: "class Parent { constructor(value) { this.base = value; } value() { return this.base; } }", lowered: "TypeError", standard: fields },
  { name: "null parent", setup: "var Parent = null;", lowered: "TypeError", standard: "TypeError" },
  { name: "overridden apply", setup: `${ordinary} Parent.apply = function(self) { self.base = 99; };`, lowered: '[99,"main","main",99]', standard: fields },
];
async function observe(code, setup) {
  const context = vm.createContext({ exports: {}, require(name) { assert.equal(name, "tslib"); return tslib; } });
  vm.runInContext(setup, context);
  let Constructor;
  if (/(?:^|[;\n])\s*(?:import|export)\b/.test(code)) {
    const module = new vm.SourceTextModule(code, { context });
    await module.link(name => {
      assert.equal(name, "tslib");
      return new vm.SyntheticModule(Object.keys(tslib), function() {
        for (const [key, value] of Object.entries(tslib)) this.setExport(key, value);
      }, { context });
    });
    await module.evaluate();
    Constructor = module.namespace.Child;
  } else {
    vm.runInContext(code, context);
    Constructor = context.exports.Child;
  }
  try {
    const value = new Constructor(41);
    const read = value.read;
    // `read` must see the constructed object even when called detached.
    return JSON.stringify([value.base, value.label, read(), value.value() - 1]);
  } catch (error) { return error.name; }
}
(async () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "wakaru-fields-"));
  function recover(code, level) {
    const input = path.join(temp, "input.js"), output = path.join(temp, "output.js");
    fs.writeFileSync(input, code);
    execFileSync(binary, [input, "--level", level, "-o", output, "--force"], { stdio: "pipe" });
    return fs.readFileSync(output, "utf8");
  }
  try {
    const files = fs.readdirSync(__dirname).filter(name => name.endsWith(".js"));
    assert.equal(files.length, 9);
    for (const file of files) {
      const lowered = fs.readFileSync(path.join(__dirname, file), "utf8");
      const minimal = recover(lowered, "minimal"), standard = recover(lowered, "standard");
      assert.match(standard, /class Child extends Parent/, `${file}: standard recovers the class`);
      for (const test of cases) {
        assert.equal(await observe(lowered, test.setup), test.lowered, `${file}: ${test.name}, lowered`);
        assert.equal(await observe(minimal, test.setup), test.lowered, `${file}: ${test.name}, minimal`);
        assert.equal(await observe(standard, test.setup), test.standard, `${file}: ${test.name}, standard`);
        assert.equal(await observe(native, test.setup), test.standard, `${file}: ${test.name}, native`);
      }
    }
    console.log("45 compiler/profile runtime cases passed");
  } finally { fs.rmSync(temp, { recursive: true, force: true }); }
})().catch(error => { console.error(error); process.exitCode = 1; });
