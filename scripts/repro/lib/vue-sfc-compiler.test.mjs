import assert from "node:assert/strict";
import test from "node:test";
import {
  compileVueSfc,
  VUE_SFC_COMPILE_PROFILES,
  vueSfcCompileProfile,
} from "./vue-sfc-compiler.mjs";

function fakeCompiler({ scriptSetup = true, template = {} } = {}) {
  const calls = { script: [], template: [] };
  return {
    calls,
    parse() {
      return {
        errors: [],
        descriptor: {
          script: scriptSetup ? null : {},
          scriptSetup: scriptSetup ? {} : null,
          template: { content: "<p>{{ count }}</p>", ...template },
        },
      };
    },
    compileScript(_descriptor, options) {
      calls.script.push(options);
      return {
        content: options.inlineTemplate
          ? "const __sfc__ = { setup() { return (_ctx, _cache) => null } }"
          : "const __sfc__ = { setup() { return {} } }",
        bindings: { count: "setup-ref" },
      };
    },
    compileTemplate(options) {
      calls.template.push(options);
      return { code: "function render() {}", errors: [] };
    },
  };
}

test("defines production-default, production-fallback, and development profiles", () => {
  assert.deepEqual(
    VUE_SFC_COMPILE_PROFILES.map(({ name, isProd, inlineTemplate }) => ({
      name,
      isProd,
      inlineTemplate,
    })),
    [
      { name: "prod-inline", isProd: true, inlineTemplate: true },
      { name: "prod-external", isProd: true, inlineTemplate: false },
      { name: "dev-external", isProd: false, inlineTemplate: false },
    ],
  );
  assert.throws(
    () => vueSfcCompileProfile("unknown"),
    /unknown Vue SFC compile profile unknown/,
  );
});

test("keeps normal script templates external in the production inline profile", () => {
  const compiler = fakeCompiler({ scriptSetup: false });
  const output = compileVueSfc({
    source: "<script>export default {}</script><template><p /></template>",
    filename: "App.vue",
    compiler,
    profile: vueSfcCompileProfile("prod-inline"),
    id: "data-v-test",
  });

  assert.equal(compiler.calls.script[0].inlineTemplate, true);
  assert.equal(compiler.calls.template[0].isProd, true);
  assert.match(output, /__sfc__\.render = render/);
});

test("rejects unresolved external and preprocessed template sources", () => {
  for (const template of [{ src: "./template.html" }, { lang: "pug" }]) {
    assert.throws(
      () => compileVueSfc({
        source: "<script setup></script><template />",
        filename: "App.vue",
        compiler: fakeCompiler({ template }),
        profile: vueSfcCompileProfile("prod-external"),
        id: "data-v-test",
      }),
      /require resolved plain template content/,
    );
  }
});

test("compiles the default production profile with an inline template", () => {
  const compiler = fakeCompiler();
  const output = compileVueSfc({
    source: "<script setup></script><template><p /></template>",
    filename: "App.vue",
    compiler,
    profile: vueSfcCompileProfile("prod-inline"),
    id: "data-v-test",
    includeFilename: true,
  });

  assert.equal(compiler.calls.script[0].isProd, true);
  assert.equal(compiler.calls.script[0].inlineTemplate, true);
  assert.equal(compiler.calls.script[0].templateOptions.isProd, true);
  assert.equal(compiler.calls.template.length, 0);
  assert.doesNotMatch(output, /\.render = render/);
  assert.match(output, /__sfc__\.__file = "App\.vue"/);
});

test("compiles external profiles with matching script and template modes", () => {
  for (const name of ["prod-external", "dev-external"]) {
    const compiler = fakeCompiler();
    const profile = vueSfcCompileProfile(name);
    const output = compileVueSfc({
      source: "<script setup></script><template><p /></template>",
      filename: "App.vue",
      compiler,
      profile,
      id: "data-v-test",
    });

    assert.equal(compiler.calls.script[0].isProd, profile.isProd);
    assert.equal(compiler.calls.script[0].inlineTemplate, false);
    assert.equal(compiler.calls.template[0].isProd, profile.isProd);
    assert.equal(
      compiler.calls.template[0].compilerOptions.bindingMetadata.count,
      "setup-ref",
    );
    assert.match(output, /__sfc__\.render = render/);
  }
});
