System.register([], function (exports_1, context_1) {
    "use strict";
    var named;
    var __moduleName = context_1 && context_1.id;
    function greet(name) {
        return `hi ${name}`;
    }
    exports_1("default", greet);
    return {
        setters: [],
        execute: function () {
            exports_1("named", named = 41);
        }
    };
});
