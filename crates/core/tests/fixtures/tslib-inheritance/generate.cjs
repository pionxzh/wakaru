const fs = require("node:fs");
const path = require("node:path");
const ts = require("typescript");
const terser = require("terser");
(async () => {
  // With downlevelIteration, tsc lowers a field-only subclass through an
  // explicit `constructor(...args)` instead of the default frame.
  for (const [directory, downlevelIteration] of [
    [__dirname, true],
    [path.join(__dirname, "static-factory"), true],
    [path.join(__dirname, "fields"), false],
  ]) {
    const source = fs.readFileSync(path.join(directory, "source.ts"), "utf8");
    for (const [name, module, importHelpers] of [
      ["commonjs-inline", ts.ModuleKind.CommonJS, false],
      ["commonjs-import-helpers", ts.ModuleKind.CommonJS, true],
      ["esm-import-helpers", ts.ModuleKind.ESNext, true],
    ]) {
      const code = ts.transpileModule(source, {
        compilerOptions: { target: ts.ScriptTarget.ES5, module, importHelpers,
          downlevelIteration, esModuleInterop: true },
      }).outputText;
      fs.writeFileSync(path.join(directory, `${name}.js`), code);
      for (const mangle of [false, true]) {
        const result = await terser.minify(code, { module: true, compress: { defaults: true, unused: false }, mangle, format: { comments: false } });
        fs.writeFileSync(path.join(directory, `${name}-${mangle ? "mangled" : "compressed"}.js`), result.code + "\n");
      }
    }
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
