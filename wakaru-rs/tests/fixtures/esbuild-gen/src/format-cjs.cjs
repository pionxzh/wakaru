exports.padLeft = function(str, len, ch) { return String(ch || ' ').repeat(Math.max(0, len - str.length)) + str; };
exports.padRight = function(str, len, ch) { return str + String(ch || ' ').repeat(Math.max(0, len - str.length)); };
