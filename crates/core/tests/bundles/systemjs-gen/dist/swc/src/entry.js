System.register([
    "./dep.js"
], function(_export, _context) {
    "use strict";
    var greet, named, value;
    async function run() {
        const mod = await _context.import("./lazy.js");
        return greet(value + mod.extra + _context.meta.url.length);
    }
    _export("run", run);
    return {
        setters: [
            function(_dep) {
                greet = _dep.default;
                named = _dep.named;
            }
        ],
        execute: function() {
            _export("value", value = named + 1);
        }
    };
});
