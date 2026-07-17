const fs = require("node:fs");
const path = require("node:path");
const { Readable } = require("node:stream");
const browserPack = require("browser-pack");

const prelude = `function e(modules, cache, entries) {
  function load(id, jumped) {
    if (!cache[id]) {
      if (!modules[id]) {
        var parts = id.split("/");
        var basename = parts[parts.length - 1];
        if (!modules[basename]) {
          var currentRequire = typeof __require == "function" && __require;
          if (!jumped && currentRequire) return currentRequire(basename, true);
          if (previousRequire) return previousRequire(basename, true);
          throw new Error("Cannot find module '" + id + "'");
        }
        id = basename;
      }
      var module = cache[id] = { exports: {} };
      modules[id][0].call(module.exports, function(request) {
        return load(modules[id][1][request] || request);
      }, module, module.exports, e, modules, cache, entries);
    }
    return cache[id].exports;
  }
  var previousRequire = typeof __require == "function" && __require;
  for (var i = 0; i < entries.length; i++) load(entries[i]);
  return load;
}`;

const sources = path.join(__dirname, "src");
const rows = [
  {
    id: "UIBase",
    source: fs.readFileSync(path.join(sources, "UIBase.js"), "utf8"),
    deps: {},
  },
  {
    id: "SampleActivityBase",
    source: fs.readFileSync(path.join(sources, "SampleActivityBase.js"), "utf8"),
    deps: { "../UIBase": "UIBase" },
  },
  {
    id: "SampleActivityBinder",
    source: fs.readFileSync(path.join(sources, "SampleActivityBinder.js"), "utf8"),
    deps: { "./SampleActivityBase": "SampleActivityBase", cc: "cc" },
    entry: true,
    order: 0,
  },
];

fs.mkdirSync(path.join(__dirname, "dist"), { recursive: true });
const output = fs.createWriteStream(path.join(__dirname, "dist", "project.js"));
const pack = browserPack({
  raw: true,
  prelude,
  hasExports: true,
  externalRequireName: "window.__require",
});

Readable.from(rows, { objectMode: true }).pipe(pack).pipe(output);
