System.register(["./dep"], function (exports_1, context_1) {
    "use strict";
    var dep_1, value;
    var __moduleName = context_1 && context_1.id;
    function run() {
        return dep_1.default(String(value));
    }
    exports_1("default", run);
    return {
        setters: [
            function (dep_1_1) {
                dep_1 = dep_1_1;
            }
        ],
        execute: function () {
            exports_1("value", value = dep_1.named + 1);
        }
    };
});
