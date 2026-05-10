exports.isEmail = function(s) { return /^[^@]+@[^@]+$/.test(s); };
exports.isNumber = function(s) { return !isNaN(parseFloat(s)); };
