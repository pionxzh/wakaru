import transform from '../un-optional-chaining'
import { defineInlineTest } from './test-utils'

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
var _a;
a?.b;

var _c_d, _c;
c?.d?.e;

var _f;
f?.g();

var _h_i, _h;
h?.i?.();

var _j_k_l_m, _j;
(j?.k.l.m)?.n.o;

// Minified
var _am;
am?.b;

var _cm;
cm?.d?.e;
`,
)

inlineTest('Babel - Object access',
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
var _foo, _a, _a$b, _a$b$c, _orders, _orders2, _client, _orders$client$key, _a2, _c, _a3;
foo?.bar;
(a?.b.c)?.d.e;
((a.b)?.c.d)?.e;
(a.b.c)?.d?.e;
orders?.[0].price;
orders?.[0]?.price;
orders[client?.key].price;
(orders[client.key])?.price;
(0, a?.b).c;
(0, ((0, a?.b).c)?.d).e;
`,
)

inlineTest('Babel - Function call',
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
var _foo, _foo2, _foo$bar, _foo3, _foo4, _foo4$bar, _foo5, _foo6, _foo$bar2, _foo7, _foo$bar3, _foo8, _foo9, _foo9$bar, _foo10, _foo10$bar;
foo?.(foo);
foo?.bar();
(foo.bar)?.(foo.bar, false);
foo?.bar?.(foo.bar, true);
foo?.().bar;
foo?.()?.bar;
(foo.bar)?.().baz;
(foo.bar)?.()?.baz;
foo?.bar?.().baz;
foo?.bar?.()?.baz;
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

var _a;
a?.b?.c;
`,
)
