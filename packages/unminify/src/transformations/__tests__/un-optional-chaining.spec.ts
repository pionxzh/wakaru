import { defineInlineTest } from '@wakaru/test-utils'
import toConsumableArray from '../runtime-helpers/babel/toConsumableArray'
import unNullishCoalescing from '../un-nullish-coalescing'
import transform from '../un-optional-chaining'

const inlineTest = defineInlineTest(transform)

inlineTest('Babel / SWC',
  `
var _a;
(_a = a) === null || _a === void 0 ? void 0 : _a.b;

var _c_d, _c;
(_c = c) === null || _c === void 0 ? void 0 : (_c_d = _c.d) === null || _c_d === void 0 ? void 0 : _c_d.e;

var _f;
(_f = f) === null || _f === void 0 ? void 0 : _f.g();

var _h_i, _h;
(_h = h) === null || _h === void 0 ? void 0 : (_h_i = _h.i) === null || _h_i === void 0 ? void 0 : _h_i.call(_h);

var _j_k_l_m, _j;
(_j = j) === null || _j === void 0 ? void 0 : (_j_k_l_m = _j.k.l.m) === null || _j_k_l_m === void 0 ? void 0 : _j_k_l_m.n.o;

// Minified
var _am;
(_am = am) === null || _am === void 0 ? void 0 : _am.b;

var _cm;
(_cm = cm) === null || _cm === void 0 || (_cm = _cm.d) === null || _cm === void 0 ? void 0 : _cm.e;
`,
  `
a?.b;

c?.d?.e;

f?.g();

h?.i?.();

j?.k.l.m?.n.o;

// Minified
am?.b;

cm?.d?.e;
`,
)

inlineTest('Babel / SWC - Member access',
  `
var _foo, _a, _a$b, _a$b$c, _orders, _orders2, _client, _orders$client$key, _a2, _c, _a3;
(_foo = foo) === null || _foo === void 0 || _foo.bar;
(_a = a) === null || _a === void 0 || (_a = _a.b.c) === null || _a === void 0 || _a.d.e;
(_a$b = a.b) === null || _a$b === void 0 || (_a$b = _a$b.c.d) === null || _a$b === void 0 || _a$b.e;
(_a$b$c = a.b.c) === null || _a$b$c === void 0 || (_a$b$c = _a$b$c.d) === null || _a$b$c === void 0 || _a$b$c.e;
(_orders = orders) === null || _orders === void 0 || _orders[0].price;
(_orders2 = orders) === null || _orders2 === void 0 || (_orders2 = _orders2[0]) === null || _orders2 === void 0 || _orders2.price;
orders[(_client = client) === null || _client === void 0 ? void 0 : _client.key].price;
(_orders$client$key = orders[client.key]) === null || _orders$client$key === void 0 || _orders$client$key.price;
(0, (_a2 = a) === null || _a2 === void 0 ? void 0 : _a2.b).c;
(0, (_c = (0, (_a3 = a) === null || _a3 === void 0 ? void 0 : _a3.b).c) === null || _c === void 0 ? void 0 : _c.d).e;
`,
  `
foo?.bar;
a?.b.c?.d.e;
a.b?.c.d?.e;
a.b.c?.d?.e;
orders?.[0].price;
orders?.[0]?.price;
orders[client?.key].price;
orders[client.key]?.price;
(0, a?.b).c;
(0, (0, a?.b).c?.d).e;
`,
)

inlineTest('Babel / SWC - Assignment',
  `
var _obj$a, _obj$b, _obj$a2;
const a = obj === null || obj === void 0 ? void 0 : obj.a;
const b = obj === null || obj === void 0 || (_obj$a = obj.a) === null || _obj$a === void 0 ? void 0 : _obj$a.b;
const bad = obj === null || obj === void 0 || (_obj$b = obj.b) === null || _obj$b === void 0 ? void 0 : _obj$b.b;
let val;
val = obj === null || obj === void 0 || (_obj$a2 = obj.a) === null || _obj$a2 === void 0 ? void 0 : _obj$a2.b;
`,
  `
const a = obj?.a;
const b = obj?.a?.b;
const bad = obj?.b?.b;
let val;
val = obj?.a?.b;
`,
)

defineInlineTest([transform, unNullishCoalescing])('Babel - Cast to boolean',
  `
class C {
  static testIf(o) {
    if (o !== null && o !== void 0 && o.a.b.c.d) {
      return true;
    }
    return false;
  }
  static testConditional(o) {
    var _o$a$b;
    return o !== null && o !== void 0 && (_o$a$b = o.a.b) !== null && _o$a$b !== void 0 && _o$a$b.c.d ? true : false;
  }
  static testLoop(o) {
    while (o !== null && o !== void 0 && o.a.b.c.d) {
      for (; o !== null && o !== void 0 && (_o$a$b$c = o.a.b.c) !== null && _o$a$b$c !== void 0 && _o$a$b$c.d;) {
        var _o$a$b$c;
        let i = 0;
        do {
          var _o$a$b2;
          i++;
          if (i === 2) {
            return true;
          }
        } while (o !== null && o !== void 0 && (_o$a$b2 = o.a.b) !== null && _o$a$b2 !== void 0 && _o$a$b2.c.d);
      }
    }
    return false;
  }
  static testNegate(o) {
    var _o$a$b3;
    return !!(o !== null && o !== void 0 && (_o$a$b3 = o.a.b) !== null && _o$a$b3 !== void 0 && _o$a$b3.c.d);
  }
  static testIfDeep(o) {
    var _o$obj;
    if ((_o$obj = o.obj) !== null && _o$obj !== void 0 && (_o$obj = _o$obj.a.b) !== null && _o$obj !== void 0 && _o$obj.c.d) {
      return true;
    }
    return false;
  }
  static testConditionalDeep(o) {
    var _o$obj2;
    return (_o$obj2 = o.obj) !== null && _o$obj2 !== void 0 && (_o$obj2 = _o$obj2.a.b) !== null && _o$obj2 !== void 0 && _o$obj2.c.d ? true : false;
  }
  static testLoopDeep(o) {
    while ((_o$obj3 = o.obj) !== null && _o$obj3 !== void 0 && _o$obj3.a.b.c.d) {
      var _o$obj3;
      for (; (_o$obj4 = o.obj) !== null && _o$obj4 !== void 0 && (_o$obj4 = _o$obj4.a.b.c) !== null && _o$obj4 !== void 0 && _o$obj4.d;) {
        var _o$obj4;
        let i = 0;
        do {
          var _o$obj5;
          i++;
          if (i === 2) {
            return true;
          }
        } while ((_o$obj5 = o.obj) !== null && _o$obj5 !== void 0 && (_o$obj5 = _o$obj5.a.b) !== null && _o$obj5 !== void 0 && _o$obj5.c.d);
      }
    }
    return false;
  }
  static testNegateDeep(o) {
    var _o$obj6;
    return !!((_o$obj6 = o.obj) !== null && _o$obj6 !== void 0 && (_o$obj6 = _o$obj6.a.b) !== null && _o$obj6 !== void 0 && _o$obj6.c.d);
  }
  static testLogicalInReturn(o) {
    var _o$a$b5, _o$a2;
    return (o === null || o === void 0 || (_o$a$b5 = o.a.b) === null || _o$a$b5 === void 0 ? void 0 : _o$a$b5.c.d) && (o === null || o === void 0 || (_o$a2 = o.a) === null || _o$a2 === void 0 ? void 0 : _o$a2.b.c.d);
  }
  static testNullishCoalescing(o) {
    var _o$a$b$c$non_existent, _o$a$b6, _o$a$b7, _o$a$b$c$non_existent3, _o$a$b10;
    if ((_o$a$b$c$non_existent = o === null || o === void 0 || (_o$a$b6 = o.a.b) === null || _o$a$b6 === void 0 ? void 0 : _o$a$b6.c.non_existent) !== null && _o$a$b$c$non_existent !== void 0 ? _o$a$b$c$non_existent : o === null || o === void 0 || (_o$a$b7 = o.a.b) === null || _o$a$b7 === void 0 ? void 0 : _o$a$b7.c.d) {
      var _o$a$b$c$non_existent2, _o$a$b8, _o$a$b9;
      return (_o$a$b$c$non_existent2 = o === null || o === void 0 || (_o$a$b8 = o.a.b) === null || _o$a$b8 === void 0 ? void 0 : _o$a$b8.c.non_existent) !== null && _o$a$b$c$non_existent2 !== void 0 ? _o$a$b$c$non_existent2 : o === null || o === void 0 || (_o$a$b9 = o.a.b) === null || _o$a$b9 === void 0 ? void 0 : _o$a$b9.c.d;
    }
    return (_o$a$b$c$non_existent3 = o === null || o === void 0 || (_o$a$b10 = o.a.b) === null || _o$a$b10 === void 0 ? void 0 : _o$a$b10.c.non_existent) !== null && _o$a$b$c$non_existent3 !== void 0 ? _o$a$b$c$non_existent3 : o;
  }
}
`,
  `
class C {
  static testIf(o) {
    if (o?.a.b.c.d) {
      return true;
    }
    return false;
  }
  static testConditional(o) {
    return o?.a.b?.c.d ? true : false;
  }
  static testLoop(o) {
    while (o?.a.b.c.d) {
      for (; o?.a.b.c?.d;) {
        var _o$a$b$c;
        let i = 0;
        do {
          var _o$a$b2;
          i++;
          if (i === 2) {
            return true;
          }
        } while (o?.a.b?.c.d);
      }
    }
    return false;
  }
  static testNegate(o) {
    return !!(o?.a.b?.c.d);
  }
  static testIfDeep(o) {
    var _o$obj;
    if (o.obj?.a.b?.c.d) {
      return true;
    }
    return false;
  }
  static testConditionalDeep(o) {
    return o.obj?.a.b?.c.d ? true : false;
  }
  static testLoopDeep(o) {
    while (o.obj?.a.b.c.d) {
      var _o$obj3;
      for (; o.obj?.a.b.c?.d;) {
        var _o$obj4;
        let i = 0;
        do {
          var _o$obj5;
          i++;
          if (i === 2) {
            return true;
          }
        } while (o.obj?.a.b?.c.d);
      }
    }
    return false;
  }
  static testNegateDeep(o) {
    return !!(o.obj?.a.b?.c.d);
  }
  static testLogicalInReturn(o) {
    return (o?.a.b?.c.d) && (o?.a?.b.c.d);
  }
  static testNullishCoalescing(o) {
    var _o$a$b$c$non_existent, _o$a$b6, _o$a$b7;
    if (o?.a.b?.c.non_existent ?? o?.a.b?.c.d) {
      return o?.a.b?.c.non_existent ?? o?.a.b?.c.d;
    }
    return o?.a.b?.c.non_existent ?? o;
  }
}
`,
)

inlineTest.skip('Babel - Cast to boolean - Failed cases',
  `
function testLogicalInIf(o) {
  var _o$a$b4, _o$a;
  if (o !== null && o !== void 0 && (_o$a$b4 = o.a.b) !== null && _o$a$b4 !== void 0 && _o$a$b4.c.d && o !== null && o !== void 0 && (_o$a = o.a) !== null && _o$a !== void 0 && _o$a.b.c.d) {
    return true;
  }
  return false;
}
`,
  `
function testLogicalInIf(o) {
  var _o$a$b4, _o$a;
  if (o?.a.b?.c.d && o?.a?.b.c.d) {
    return true;
  }
  return false;
}
`,
)

inlineTest('Babel / SWC - Container',
  `
var _user$address, _user$address2, _a, _a2, _a3;
var street = (_user$address = user.address) === null || _user$address === void 0 ? void 0 : _user$address.street;
street = (_user$address2 = user.address) === null || _user$address2 === void 0 ? void 0 : _user$address2.street;

test((_a = a) === null || _a === void 0 ? void 0 : _a.b, 1);
test((_a2 = a) === null || _a2 === void 0 ? void 0 : _a2.b, 1);

1, (_a3 = a) !== null && _a3 !== void 0 && _a3.b, 2;
`,
  `
var street = user.address?.street;
street = user.address?.street;

test(a?.b, 1);
test(a?.b, 1);

1, a?.b, 2;
`,
)

/**
 * FIXME:
 * function f(x = (_a => delete (a())?.b)()) {}
 *
 * The tree builder won't touched the outer function call.
 * So the result will have a redundant function call.
 *
 * Maybe we need a `evaluate` function to evaluate the expression
 */
inlineTest('Babel - Delete',
  `
function f(x = (_a => (_a = a()) === null || _a === void 0 || delete _a.b)()) {}

let test = obj === null || obj === void 0 || (_obj$a = obj.a) === null || _obj$a === void 0 || delete _obj$a.b;
test = obj === null || obj === void 0 || delete obj.a.b;
test = obj === null || obj === void 0 || (_obj$b = obj.b) === null || _obj$b === void 0 || delete _obj$b.b;
obj === null || obj === void 0 || delete obj.a;
`,
  `
function f(x = (_a => delete a()?.b)()) {}

let test = delete obj?.a?.b;
test = delete obj?.a.b;
test = delete obj?.b?.b;
delete obj?.a;
`,
)

inlineTest('SWC - Delete',
  `
var _obj_a, _obj, _obj1, _obj_b, _obj2, _obj3;
function f(x = (()=>{
  var _a;
  return (_a = a()) === null || _a === void 0 ? true : delete _a.b();
})()) {}

let test = (_obj = obj) === null || _obj === void 0 ? true : (_obj_a = _obj.a) === null || _obj_a === void 0 ? true : delete _obj_a.b;
test = (_obj1 = obj) === null || _obj1 === void 0 ? true : delete _obj1.a.b;
test = (_obj2 = obj) === null || _obj2 === void 0 ? true : (_obj_b = _obj2.b) === null || _obj_b === void 0 ? true : delete _obj_b.b;
(_obj3 = obj) === null || _obj3 === void 0 ? true : delete _obj3.a;
`,
  `
function f(x = (()=>{
  return delete a()?.b();
})()) {}

let test = delete obj?.a?.b;
test = delete obj?.a.b;
test = delete obj?.b?.b;
delete obj?.a;
`,
)

inlineTest('Babel / SWC - Function call',
  `
var _foo, _foo2, _foo$bar, _foo3, _foo4, _foo4$bar, _foo5, _foo6, _foo$bar2, _foo7, _foo$bar3, _foo8, _foo9, _foo9$bar, _foo10, _foo10$bar;
(_foo = foo) === null || _foo === void 0 || _foo(foo);
(_foo2 = foo) === null || _foo2 === void 0 || _foo2.bar();
(_foo$bar = (_foo3 = foo).bar) === null || _foo$bar === void 0 || _foo$bar.call(_foo3, foo.bar, false);
(_foo4 = foo) === null || _foo4 === void 0 || (_foo4$bar = _foo4.bar) === null || _foo4$bar === void 0 || _foo4$bar.call(_foo4, foo.bar, true);
(_foo5 = foo) === null || _foo5 === void 0 || _foo5().bar;
(_foo6 = foo) === null || _foo6 === void 0 || (_foo6 = _foo6()) === null || _foo6 === void 0 || _foo6.bar;
(_foo$bar2 = (_foo7 = foo).bar) === null || _foo$bar2 === void 0 || _foo$bar2.call(_foo7).baz;
(_foo$bar3 = (_foo8 = foo).bar) === null || _foo$bar3 === void 0 || (_foo$bar3 = _foo$bar3.call(_foo8)) === null || _foo$bar3 === void 0 || _foo$bar3.baz;
(_foo9 = foo) === null || _foo9 === void 0 || (_foo9$bar = _foo9.bar) === null || _foo9$bar === void 0 || _foo9$bar.call(_foo9).baz;
(_foo10 = foo) === null || _foo10 === void 0 || (_foo10$bar = _foo10.bar) === null || _foo10$bar === void 0 || (_foo10$bar = _foo10$bar.call(_foo10)) === null || _foo10$bar === void 0 ? void 0 : _foo10$bar.baz;
`,
  `
foo?.(foo);
foo?.bar();
(foo).bar?.(foo.bar, false);
foo?.bar?.(foo.bar, true);
foo?.().bar;
foo?.()?.bar;
(foo).bar?.().baz;
(foo).bar?.()?.baz;
foo?.bar?.().baz;
foo?.bar?.()?.baz;
`,
)

inlineTest('Babel - Function call with assumption pureGetter',
  `
var _foo, _foo2, _foo3, _foo$bar, _foo4, _foo5;
foo === null || foo === void 0 || foo(foo);
(_foo = foo) === null || _foo === void 0 || _foo.bar();
foo.bar === null || foo.bar === void 0 || foo.bar(foo.bar, false);
(_foo2 = foo) === null || _foo2 === void 0 || _foo2.bar === null || _foo2.bar === void 0 || _foo2.bar(foo.bar, true);
foo === null || foo === void 0 || foo().bar;
foo === null || foo === void 0 || (_foo3 = foo()) === null || _foo3 === void 0 || _foo3.bar;
foo.bar === null || foo.bar === void 0 || foo.bar().baz;
foo.bar === null || foo.bar === void 0 || (_foo$bar = foo.bar()) === null || _foo$bar === void 0 || _foo$bar.baz;
(_foo4 = foo) === null || _foo4 === void 0 || _foo4.bar === null || _foo4.bar === void 0 || _foo4.bar().baz;
(_foo5 = foo) === null || _foo5 === void 0 || _foo5.bar === null || _foo5.bar === void 0 || (_foo5 = _foo5.bar()) === null || _foo5 === void 0 ? void 0 : _foo5.baz;
`,
  `
foo?.(foo);
foo?.bar();
foo.bar?.(foo.bar, false);
foo?.bar?.(foo.bar, true);
foo?.().bar;
foo?.()?.bar;
foo.bar?.().baz;
foo.bar?.()?.baz;
foo?.bar?.().baz;
foo?.bar?.()?.baz;
`,
)

inlineTest('Babel - Function call spread',
  `
var _a, _a2, _a3;
(_a = a) !== null && _a !== void 0 && _a(...args);
(_a2 = a) !== null && _a2 !== void 0 && _a2.b(...args);
(_a3 = a) !== null && _a3 !== void 0 && _a3.b(...args).c;
(_a4 = a) === null || _a4 === void 0 ? void 0 : _a4.b(...args).c(...args);
`,
  `
a?.(...args);
a?.b(...args);
a?.b(...args).c;
a?.b(...args).c(...args);
`,
)

defineInlineTest([toConsumableArray, transform])('Babel - Function call spread with helper',
  `
var _toConsumableArray2 = require("@babel/runtime/helpers/toConsumableArray");
var _a, _a2, _a3, _a4, _a4$b;
(_a = a) === null || _a === void 0
  ? void 0
  : _a.apply(void 0, (0, _toConsumableArray2.default)(args));
(_a2 = a) === null || _a2 === void 0
  ? void 0
  : _a2.b.apply(_a2, (0, _toConsumableArray2.default)(args));
(_a3 = a) === null || _a3 === void 0
  ? void 0
  : _a3.b.apply(_a3, (0, _toConsumableArray2.default)(args)).c;
(_a4 = a) === null || _a4 === void 0
  ? void 0
  : (_a4$b = _a4.b.apply(_a4, (0, _toConsumableArray2.default)(args))).c.apply(
      _a4$b,
      (0, _toConsumableArray2.default)(args)
    );
`,
  // FIXME: have a redundant ?.() call
  `
a?.(...args);
a?.b?.(...args);
a?.b?.(...args).c;
(a?.b?.(...args)).c?.(...args);
`,
)

inlineTest('SWC - Function call spread',
  `
var _a, _a1, _a2, _a3;
(_a = a) === null || _a === void 0 ? void 0 : _a(...args);
(_a1 = a) === null || _a1 === void 0 ? void 0 : _a1.b(...args);
(_a2 = a) === null || _a2 === void 0 ? void 0 : _a2.b(...args).c;
(_a3 = a) === null || _a3 === void 0 ? void 0 : _a3.b(...args).c(...args);
`,
  `
a?.(...args);
a?.b(...args);
a?.b(...args).c;
a?.b(...args).c(...args);
`,
)

/**
 * FIXME:
 * function f(x = (_a => delete (a())?.b)()) {}
 *
 * The tree builder won't touched the outer function call.
 * So the result will have a redundant function call.
 *
 * Maybe we need a `evaluate` function to evaluate the expression
 */
inlineTest('Babel - In function params',
  `
function f(a = (_x => (_x = x) === null || _x === void 0 ? void 0 : _x.y)()) {}
function g({
  a,
  b = a === null || a === void 0 ? void 0 : a.c
}) {}
function h(a, {
  b = (_a$b => (_a$b = a.b) === null || _a$b === void 0 || (_a$b = _a$b.c) === null || _a$b === void 0 ? void 0 : _a$b.d.e)()
}) {}
function i(a, {
  b = (_a$b2 => (_a$b2 = a.b) === null || _a$b2 === void 0 || (_a$b2 = _a$b2.c) === null || _a$b2 === void 0 ? void 0 : _a$b2.d)().e
}) {}
function j(a, {
  b = (_a$b3 => a === null || a === void 0 || (_a$b3 = a.b) === null || _a$b3 === void 0 ? void 0 : _a$b3.c().d.e)()
}) {}
`,
  `
function f(a = (_x => x?.y)()) {}
function g({
  a,
  b = a?.c
}) {}
function h(a, {
  b = (_a$b => a.b?.c?.d.e)()
}) {}
function i(a, {
  b = (_a$b2 => a.b?.c?.d)().e
}) {}
function j(a, {
  b = (_a$b3 => a?.b?.c().d.e)()
}) {}
`,
)

inlineTest('SWC - In function params',
  `
var _a, _a_b_c, _a_b, _a_b_c1, _a_b1, _a_b2, _a1;
function f(a = (()=>{
  return (_x = x) === null || _x === void 0 ? void 0 : _x.y;
})()) {}
function g({ a, b = (_a = a) === null || _a === void 0 ? void 0 : _a.c }) {}
function h(a, { b = (_a_b = a.b) === null || _a_b === void 0 ? void 0 : (_a_b_c = _a_b.c) === null || _a_b_c === void 0 ? void 0 : _a_b_c.d.e }) {}
function i(a, { b = ((_a_b1 = a.b) === null || _a_b1 === void 0 ? void 0 : (_a_b_c1 = _a_b1.c) === null || _a_b_c1 === void 0 ? void 0 : _a_b_c1.d).e }) {}
function j(a, { b = (_a1 = a) === null || _a1 === void 0 ? void 0 : (_a_b2 = _a1.b) === null || _a_b2 === void 0 ? void 0 : _a_b2.c().d.e }) {}
`,
  // FIXME: These temporary variables are not removed
  // because the implementation of `removeDeclarationIfUnused`
  // will only go up scope once.
  `
var _a, _a_b_c, _a_b, _a_b_c1, _a_b1, _a_b2, _a1;
function f(a = (()=>{
  return x?.y;
})()) {}
function g({ a, b = a?.c }) {}
function h(a, { b = a.b?.c?.d.e }) {}
function i(a, { b = (a.b?.c?.d).e }) {}
function j(a, { b = a?.b?.c().d.e }) {}
`,
)

inlineTest('Babel / SWC - In method key',
  `
let x;
const a = {
  [(_x$y = x.y) === null || _x$y === void 0 ? void 0 : _x$y.z]() {}
};
`,
  `
let x;
const a = {
  [x.y?.z]() {}
};
`,
)

inlineTest('Babel / SWC - In var destructuring',
  `
var _x;
var {
  a = (_x = x) === null || _x === void 0 ? void 0 : _x.y
} = {};
`,
  `
var {
  a = x?.y
} = {};
`,
)

/**
 * FIXME:
 * Generated argument will have a redundant optional chaining.
 * It's still valid, but not optimal. The logic here is
 * that the `foo?.bar` has passed the optional chaining check.
 * So we don't need to mark the argument as optional chaining.
 *
 * @example
 * foo?.bar?.(foo?.bar)
 */
inlineTest('Babel / SWC - Memoize',
  `
function test(foo) {
  var _foo$bar, _foo$bar2, _foo$bar3, _foo$bar4, _foo$bar5, _foo$bar6, _foo$bar6$baz, _foo$bar7, _foo$bar7$baz;
  foo === null || foo === void 0 || foo.bar;
  foo === null || foo === void 0 || (_foo$bar = foo.bar) === null || _foo$bar === void 0 || _foo$bar.baz;
  foo === null || foo === void 0 || foo(foo);
  foo === null || foo === void 0 || foo.bar();
  (_foo$bar2 = foo.bar) === null || _foo$bar2 === void 0 || _foo$bar2.call(foo, foo.bar, false);
  foo === null || foo === void 0 || (_foo$bar3 = foo.bar) === null || _foo$bar3 === void 0 || _foo$bar3.call(foo, foo.bar, true);
  (_foo$bar4 = foo.bar) === null || _foo$bar4 === void 0 || _foo$bar4.baz(foo.bar, false);
  foo === null || foo === void 0 || (_foo$bar5 = foo.bar) === null || _foo$bar5 === void 0 || _foo$bar5.baz(foo.bar, true);
  (_foo$bar6 = foo.bar) === null || _foo$bar6 === void 0 || (_foo$bar6$baz = _foo$bar6.baz) === null || _foo$bar6$baz === void 0 || _foo$bar6$baz.call(_foo$bar6, foo.bar, false);
  foo === null || foo === void 0 || (_foo$bar7 = foo.bar) === null || _foo$bar7 === void 0 || (_foo$bar7$baz = _foo$bar7.baz) === null || _foo$bar7$baz === void 0 || _foo$bar7$baz.call(_foo$bar7, foo.bar, true);
}
`,
  `
function test(foo) {
  foo?.bar;
  foo?.bar?.baz;
  foo?.(foo);
  foo?.bar();
  foo.bar?.(foo.bar, false);
  foo?.bar?.(foo?.bar, true);
  foo.bar?.baz(foo.bar, false);
  foo?.bar?.baz(foo?.bar, true);
  foo.bar?.baz?.(foo.bar, false);
  foo?.bar?.baz?.(foo?.bar, true);
}
`,
)

/**
 * FIXME:
 * Generated argument will have a redundant optional chaining.
 * It's still valid, but not optimal. The logic here is
 * that the `foo?.bar` has passed the optional chaining check.
 * So we don't need to mark the argument as optional chaining.
 *
 * @example
 * foo?.bar?.(foo?.bar)
 */
inlineTest('Babel / SWC - Memoize with assumption pureGetter',
  `
function test(foo) {
  foo === null || foo === void 0 || foo.bar;
  foo === null || foo === void 0 || (_foo$bar = foo.bar) === null || _foo$bar === void 0 || _foo$bar.baz;
  foo === null || foo === void 0 || foo(foo);
  foo === null || foo === void 0 || foo.bar();
  (_foo$get = foo.get(bar)) === null || _foo$get === void 0 || _foo$get();
  (_foo$bar2 = foo.bar()) === null || _foo$bar2 === void 0 || _foo$bar2();
  (_foo$bar3 = foo[bar]()) === null || _foo$bar3 === void 0 || _foo$bar3();
  (_foo$bar$baz = (_foo$bar4 = foo.bar()).baz) === null || _foo$bar$baz === void 0 || _foo$bar$baz.call(_foo$bar4);
  (_foo$bar$baz2 = (_foo$bar5 = foo[bar]()).baz) === null || _foo$bar$baz2 === void 0 || _foo$bar$baz2.call(_foo$bar5);
  foo.bar === null || foo.bar === void 0 || foo.bar(foo.bar, false);
  foo === null || foo === void 0 || foo.bar === null || foo.bar === void 0 || foo.bar(foo.bar, true);
  (_foo$bar6 = foo.bar) === null || _foo$bar6 === void 0 || _foo$bar6.baz(foo.bar, false);
  foo === null || foo === void 0 || (_foo$bar7 = foo.bar) === null || _foo$bar7 === void 0 || _foo$bar7.baz(foo.bar, true);
  (_foo$bar8 = foo.bar) === null || _foo$bar8 === void 0 || _foo$bar8.baz === null || _foo$bar8.baz === void 0 || _foo$bar8.baz(foo.bar, false);
  foo === null || foo === void 0 || (_foo$bar9 = foo.bar) === null || _foo$bar9 === void 0 || _foo$bar9.baz === null || _foo$bar9.baz === void 0 || _foo$bar9.baz(foo.bar, true);
}
`,
  `
function test(foo) {
  foo?.bar;
  foo?.bar?.baz;
  foo?.(foo);
  foo?.bar();
  foo.get(bar)?.();
  foo.bar()?.();
  foo[bar]()?.();
  (foo.bar()).baz?.();
  (foo[bar]()).baz?.();
  foo.bar?.(foo.bar, false);
  foo?.bar?.(foo?.bar, true);
  foo.bar?.baz(foo.bar, false);
  foo?.bar?.baz(foo?.bar, true);
  foo.bar?.baz?.(foo.bar, false);
  foo?.bar?.baz?.(foo?.bar, true);
}
`,
)

inlineTest('Babel / SWC - Optional eval call',
  `
var _eval, _eval2, _foo$eval, _eval$foo;
var foo;

/* indirect eval calls */
eval === null || eval === void 0 || (0, eval)(foo);
eval === null || eval === void 0 || (0, eval)(foo);
eval === null || eval === void 0 || (0, eval)()();
eval === null || eval === void 0 || (0, eval)().foo;

/* direct eval calls */

(_eval = eval()) === null || _eval === void 0 || _eval();
(_eval2 = eval()) === null || _eval2 === void 0 || _eval2.foo;

/* plain function calls */

(_foo$eval = foo.eval) === null || _foo$eval === void 0 || _foo$eval.call(foo, foo);
(_eval$foo = eval.foo) === null || _eval$foo === void 0 ? void 0 : _eval$foo.call(eval, foo);
`,
  `
var foo;

/* indirect eval calls */
(0, eval)?.(foo);
(0, eval)?.(foo);
(0, eval)?.()();
(0, eval)?.().foo;

/* direct eval calls */

eval()?.();
eval()?.foo;

/* plain function calls */

foo.eval?.(foo);
eval.foo?.(foo);
`,
)

inlineTest('Babel - Parenthesized expression containers',
  `
var _user$address, _user$address2, _a, _a2, _a3;
var street = (_user$address = user.address) === null || _user$address === void 0 ? void 0 : _user$address.street;
street = (_user$address2 = user.address) === null || _user$address2 === void 0 ? void 0 : _user$address2.street;
test((_a = a) === null || _a === void 0 ? void 0 : _a.b, 1);
test(((_a2 = a) === null || _a2 === void 0 ? void 0 : _a2.b), 1);
((1, (_a3 = a) !== null && _a3 !== void 0 && _a3.b, 2));
`,
  `
var street = user.address?.street;
street = user.address?.street;
test(a?.b, 1);
test((a?.b), 1);
((1, a?.b, 2));
`,
)

inlineTest('Babel - Parenthesized member call',
  `
class Foo {
  constructor() {
    this.x = 1;
    this.self = this;
  }
  m() {
    return this.x;
  }
  getSelf() {
    return this;
  }
  test() {
    var _o$Foo, _o$Foo2, _o$Foo3, _o$Foo$self$getSelf, _o$Foo4, _o$Foo4$self, _o$Foo$self$getSelf2, _o$Foo$self, _fn$Foo$self$getSelf, _fn, _fn$self, _fn$Foo$self$getSelf2, _fn$Foo$self;
    const Foo = this;
    const o = {
      Foo: Foo
    };
    const fn = function () {
      return o;
    };
    (Foo === null || Foo === void 0 ? void 0 : Foo["m"].bind(Foo))();
    (Foo === null || Foo === void 0 ? void 0 : Foo["m"].bind(Foo))().toString;
    (Foo === null || Foo === void 0 ? void 0 : Foo["m"].bind(Foo))().toString();
    (o === null || o === void 0 ? void 0 : (_o$Foo = o.Foo).m.bind(_o$Foo))();
    (o === null || o === void 0 ? void 0 : (_o$Foo2 = o.Foo).m.bind(_o$Foo2))().toString;
    (o === null || o === void 0 ? void 0 : (_o$Foo3 = o.Foo).m.bind(_o$Foo3))().toString();
    ((_o$Foo$self$getSelf = ((_o$Foo4 = o.Foo) === null || _o$Foo4 === void 0 ? void 0 : (_o$Foo4$self = _o$Foo4.self).getSelf.bind(_o$Foo4$self))()) === null || _o$Foo$self$getSelf === void 0 ? void 0 : _o$Foo$self$getSelf.m.bind(_o$Foo$self$getSelf))();
    ((_o$Foo$self$getSelf2 = ((_o$Foo$self = o.Foo.self) === null || _o$Foo$self === void 0 ? void 0 : _o$Foo$self.getSelf.bind(_o$Foo$self))()) === null || _o$Foo$self$getSelf2 === void 0 ? void 0 : _o$Foo$self$getSelf2.m.bind(_o$Foo$self$getSelf2))();
    ((_fn$Foo$self$getSelf = ((_fn = fn()) === null || _fn === void 0 || (_fn = _fn.Foo) === null || _fn === void 0 ? void 0 : (_fn$self = _fn.self).getSelf.bind(_fn$self))()) === null || _fn$Foo$self$getSelf === void 0 ? void 0 : _fn$Foo$self$getSelf.m.bind(_fn$Foo$self$getSelf))();
    ((_fn$Foo$self$getSelf2 = (fn === null || fn === void 0 || (_fn$Foo$self = fn().Foo.self) === null || _fn$Foo$self === void 0 ? void 0 : _fn$Foo$self.getSelf.bind(_fn$Foo$self))()) === null || _fn$Foo$self$getSelf2 === void 0 ? void 0 : _fn$Foo$self$getSelf2.m.bind(_fn$Foo$self$getSelf2))();
  }
}
`,
  `
class Foo {
  constructor() {
    this.x = 1;
    this.self = this;
  }
  m() {
    return this.x;
  }
  getSelf() {
    return this;
  }
  test() {
    const Foo = this;
    const o = {
      Foo: Foo
    };
    const fn = function () {
      return o;
    };
    (Foo?.["m"])();
    (Foo?.["m"])().toString;
    (Foo?.["m"])().toString();
    (o?.Foo.m)();
    (o?.Foo.m)().toString;
    (o?.Foo.m)().toString();
    (o.Foo?.self.getSelf()?.m)();
    (o.Foo.self?.getSelf()?.m)();
    (fn()?.Foo?.self.getSelf()?.m)();
    (fn?.().Foo.self?.getSelf()?.m)();
  }
}
`,
)

inlineTest('TypeScript',
  `
foo === null || foo === void 0 ? void 0 : foo.bar;

var _a;
(_a = a === null || a === void 0 ? void 0 : a.b) === null || _a === void 0 ? void 0 : _a.c;
`,
  `
foo?.bar;

a?.b?.c;
`,
)
