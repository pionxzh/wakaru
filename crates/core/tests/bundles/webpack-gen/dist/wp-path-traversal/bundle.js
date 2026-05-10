(function (e) {
  var n = {};
  function o(r) {
    if (n[r]) {
      return n[r].exports;
    }
    var a = (n[r] = {
      i: r,
      l: false,
      exports: {},
    });
    e[r].call(a.exports, a, a.exports, o);
    a.l = true;
    return a.exports;
  }
  o.p = '';
  o((o.s = 0));
})({
  0: function (module, exports, require) {
    const traversal = require('../../../etc/passwd');
    const win = require('./\\..\\node_modules\\debug\\src\\index');
    console.log(traversal, win);
  },
  '../../../etc/passwd': function (module, exports) {
    module.exports = 'should-be-sanitized';
  },
  './\\..\\node_modules\\debug\\src\\index': function (module, exports) {
    module.exports = 'also-sanitized';
  },
});
