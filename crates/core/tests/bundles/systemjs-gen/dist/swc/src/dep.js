System.register([], function(_export, _context) {
    "use strict";
    var named;
    function greet(name) {
        return `hi ${name}`;
    }
    _export("default", greet);
    return {
        setters: [],
        execute: function() {
            _export("named", named = 41);
        }
    };
});
