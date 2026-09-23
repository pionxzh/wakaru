//! `for await` recovery (`rules/un_for_await.rs`, run by `UnForOf`).
//!
//! Inputs are the async-function shapes the pipeline reaches after
//! `UnAsyncAwait` restored `await`: the adapter helper declaration plus the
//! try/catch/finally iteration protocol. `render` runs the full pipeline so
//! `UnVariableMerging` and `UnConditionals` reshape the loop head and guards
//! exactly as they do for real bundles.

mod common;

use common::{assert_eq_normalized, render};

const BABEL_ASYNC_ITERATOR: &str = r#"
function _asyncIterator(r) { var n, t, o, e = 2; for ("undefined" != typeof Symbol && (t = Symbol.asyncIterator, o = Symbol.iterator); e--;) { if (t && null != (n = r[t])) return n.call(r); if (o && null != (n = r[o])) return new AsyncFromSyncIterator(n.call(r)); t = "@@asyncIterator", o = "@@iterator"; } throw new TypeError("Object is not async iterable"); }
function AsyncFromSyncIterator(r) { function AsyncFromSyncIteratorContinuation(r) { if (Object(r) !== r) return Promise.reject(new TypeError(r + " is not an object.")); var n = r.done; return Promise.resolve(r.value).then(function (r) { return { value: r, done: n }; }); } return AsyncFromSyncIterator = function (r) { this.s = r, this.n = r.next; }, AsyncFromSyncIterator.prototype = { s: null, n: null, next: function () { return AsyncFromSyncIteratorContinuation(this.n.apply(this.s, arguments)); }, return: function (r) { var n = this.s.return; return void 0 === n ? Promise.resolve({ value: r, done: !0 }) : AsyncFromSyncIteratorContinuation(n.apply(this.s, arguments)); }, throw: function (r) { var n = this.s.return; return void 0 === n ? Promise.reject(r) : AsyncFromSyncIteratorContinuation(n.apply(this.s, arguments)); } }, new AsyncFromSyncIterator(r); }
"#;

const OLD_BABEL_ASYNC_ITERATOR: &str = r#"
function _asyncIterator(iterable) { var method; if (typeof Symbol !== "undefined") { if (Symbol.asyncIterator) { method = iterable[Symbol.asyncIterator]; if (method != null) return method.call(iterable); } if (Symbol.iterator) { method = iterable[Symbol.iterator]; if (method != null) return method.call(iterable); } } throw new TypeError("Object is not async iterable"); }
"#;

const ESBUILD_FOR_AWAIT: &str = r#"
var __knownSymbol = (name, symbol) => (symbol = Symbol[name]) ? symbol : Symbol.for("Symbol." + name);
var __forAwait = (obj, it, method) => (it = obj[__knownSymbol("asyncIterator")]) ? it.call(obj) : (obj = obj[__knownSymbol("iterator")](), it = {}, method = (key, fn) => (fn = obj[key]) && (it[key] = (arg) => new Promise((yes, no, done) => (arg = fn.call(obj, arg), done = arg.done, Promise.resolve(arg.value).then((value) => yes({ value, done }), no)))), method("next"), method("return"), it);
"#;

const TS_ASYNC_VALUES: &str = r#"
var __asyncValues = (this && this.__asyncValues) || function (o) {
    if (!Symbol.asyncIterator) throw new TypeError("Symbol.asyncIterator is not defined.");
    var m = o[Symbol.asyncIterator], i;
    return m ? m.call(o) : (o = typeof __values === "function" ? __values(o) : o[Symbol.iterator](), i = {}, verb("next"), verb("throw"), verb("return"), i[Symbol.asyncIterator] = function () { return this; }, i);
    function verb(n) { i[n] = o[n] && function (v) { return new Promise(function (resolve, reject) { v = o[n](v), settle(resolve, reject, v.done, v.value); }); }; }
    function settle(resolve, reject, d, v) { Promise.resolve(v).then(function(v) { resolve({ value: v, done: d }); }, reject); }
};
"#;

fn with_helper(helper: &str, body: &str) -> String {
    format!("{helper}\n{body}")
}

#[test]
fn for_await_from_babel_async_iterator() {
    // @babel/plugin-transform-async-generator-functions ≥ 7.28 after
    // async-to-generator recovery. The original loop body is nested as a block.
    let input = with_helper(
        BABEL_ASYNC_ITERATOR,
        r#"
async function consume(stream) {
  var _iteratorAbruptCompletion = false;
  var _didIteratorError = false;
  var _iteratorError;
  try {
    for (var _iterator = _asyncIterator(stream), _step; _iteratorAbruptCompletion = !(_step = await _iterator.next()).done; _iteratorAbruptCompletion = false) {
      const item = _step.value;
      {
        if (item.done) break;
        await handle(item);
      }
    }
  } catch (err) {
    _didIteratorError = true;
    _iteratorError = err;
  } finally {
    try {
      if (_iteratorAbruptCompletion && _iterator.return != null) {
        await _iterator.return();
      }
    } finally {
      if (_didIteratorError) {
        throw _iteratorError;
      }
    }
  }
}
"#,
    );
    let expected = r#"
async function consume(stream) {
  for await (const item of stream) {
    if (item.done) {
      break;
    }
    await handle(item);
  }
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_from_old_babel_async_iterator_awaits_value() {
    // Babel 7.8–7.13: the head also awaits `step.value` and tracks a
    // normal-completion flag that starts true.
    let input = with_helper(
        OLD_BABEL_ASYNC_ITERATOR,
        r#"
async function consume(stream) {
  var _iteratorNormalCompletion = true;
  var _didIteratorError = false;
  var _iteratorError;
  try {
    for (var _iterator = _asyncIterator(stream), _step, _value; _step = await _iterator.next(), _iteratorNormalCompletion = _step.done, _value = await _step.value, !_iteratorNormalCompletion; _iteratorNormalCompletion = true) {
      const item = _value;
      await handle(item);
    }
  } catch (err) {
    _didIteratorError = true;
    _iteratorError = err;
  } finally {
    try {
      if (!_iteratorNormalCompletion && _iterator.return != null) {
        await _iterator.return();
      }
    } finally {
      if (_didIteratorError) {
        throw _iteratorError;
      }
    }
  }
}
"#,
    );
    let expected = r#"
async function consume(stream) {
  for await (const item of stream) {
    await handle(item);
  }
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_from_esbuild_for_await_helper() {
    // esbuild (target < es2018) after `__async` recovery. The catch parameter
    // reuses the `temp` name and the close guard assigns the method temp.
    let input = with_helper(
        ESBUILD_FOR_AWAIT,
        r#"
async function consume(stream) {
  const output = [];
  try {
    for (var iter = __forAwait(stream), more, temp, error; more = !(temp = await iter.next()).done; more = false) {
      const item = temp.value;
      output.push(await normalize(item));
    }
  } catch (temp) {
    error = [temp];
  } finally {
    try {
      more && (temp = iter.return) && (await temp.call(iter));
    } finally {
      if (error)
        throw error[0];
    }
  }
  return output;
}
"#,
    );
    let expected = r#"
async function consume(stream) {
  const output = [];
  for await (const item of stream) {
    output.push(await normalize(item));
  }
  return output;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_from_ts_async_values() {
    // TypeScript 5 (`downlevelIteration`), written as the shape reached once
    // the `__generator` state machine is restored: a `first` flag, a done
    // temporary, and the value copied through a temporary.
    let input = with_helper(
        TS_ASYNC_VALUES,
        r#"
async function consume(stream) {
  var e_1, _a, _b, _c;
  try {
    for (var _d = true, stream_1 = __asyncValues(stream), stream_1_1; stream_1_1 = await stream_1.next(), _a = stream_1_1.done, !_a; _d = true) {
      _c = stream_1_1.value;
      _d = false;
      const item = _c;
      await handle(item);
    }
  } catch (e_1_1) {
    e_1 = { error: e_1_1 };
  } finally {
    try {
      if (!_d && !_a && (_b = stream_1.return)) await _b.call(stream_1);
    } finally {
      if (e_1) throw e_1.error;
    }
  }
}
"#,
    );
    let expected = r#"
async function consume(stream) {
  for await (const item of stream) {
    await handle(item);
  }
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_from_ts4_async_values() {
    // TypeScript 4: no first flag, the close guard reads the step itself.
    let input = with_helper(
        TS_ASYNC_VALUES,
        r#"
async function consume(stream) {
  var e_1, _a;
  try {
    for (var stream_1 = __asyncValues(stream), stream_1_1; stream_1_1 = await stream_1.next(), !stream_1_1.done;) {
      const item = stream_1_1.value;
      await handle(item);
    }
  } catch (e_1_1) {
    e_1 = { error: e_1_1 };
  } finally {
    try {
      if (stream_1_1 && !stream_1_1.done && (_a = stream_1.return)) await _a.call(stream_1);
    } finally {
      if (e_1) throw e_1.error;
    }
  }
}
"#,
    );
    let expected = r#"
async function consume(stream) {
  for await (const item of stream) {
    await handle(item);
  }
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_recovers_destructured_element() {
    let input = with_helper(
        BABEL_ASYNC_ITERATOR,
        r#"
async function index(records) {
  const index = new Map();
  var _iteratorAbruptCompletion = false;
  var _didIteratorError = false;
  var _iteratorError;
  try {
    for (var _iterator = _asyncIterator(records), _step; _iteratorAbruptCompletion = !(_step = await _iterator.next()).done; _iteratorAbruptCompletion = false) {
      const { id, value } = _step.value;
      {
        index.set(id, value);
      }
    }
  } catch (err) {
    _didIteratorError = true;
    _iteratorError = err;
  } finally {
    try {
      if (_iteratorAbruptCompletion && _iterator.return != null) {
        await _iterator.return();
      }
    } finally {
      if (_didIteratorError) {
        throw _iteratorError;
      }
    }
  }
  return index;
}
"#,
    );
    let expected = r#"
async function index(records) {
  const index = new Map();
  for await (const { id, value } of records) {
    index.set(id, value);
  }
  return index;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_from_imported_babel_helper_removes_import() {
    let input = r#"
import _asyncIterator from "@babel/runtime/helpers/asyncIterator";
export async function consume(stream) {
  var _iteratorAbruptCompletion = false;
  var _didIteratorError = false;
  var _iteratorError;
  try {
    for (var _iterator = _asyncIterator(stream), _step; _iteratorAbruptCompletion = !(_step = await _iterator.next()).done; _iteratorAbruptCompletion = false) {
      const item = _step.value;
      await handle(item);
    }
  } catch (err) {
    _didIteratorError = true;
    _iteratorError = err;
  } finally {
    try {
      if (_iteratorAbruptCompletion && _iterator.return != null) {
        await _iterator.return();
      }
    } finally {
      if (_didIteratorError) {
        throw _iteratorError;
      }
    }
  }
}
"#;
    let expected = r#"
export async function consume(stream) {
  for await (const item of stream) {
    await handle(item);
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_await_keeps_helper_with_remaining_call_site() {
    // A second, unrecovered adapter call keeps the helper declaration alive.
    let input = with_helper(
        OLD_BABEL_ASYNC_ITERATOR,
        r#"
export function open(stream) {
  return _asyncIterator(stream);
}
export async function consume(stream) {
  var _iteratorAbruptCompletion = false;
  var _didIteratorError = false;
  var _iteratorError;
  try {
    for (var _iterator = _asyncIterator(stream), _step; _iteratorAbruptCompletion = !(_step = await _iterator.next()).done; _iteratorAbruptCompletion = false) {
      const item = _step.value;
      await handle(item);
    }
  } catch (err) {
    _didIteratorError = true;
    _iteratorError = err;
  } finally {
    try {
      if (_iteratorAbruptCompletion && _iterator.return != null) {
        await _iterator.return();
      }
    } finally {
      if (_didIteratorError) {
        throw _iteratorError;
      }
    }
  }
}
"#,
    );
    let output = render(&input);
    assert!(
        output.contains("for await (const item of stream)"),
        "loop not recovered:\n{output}"
    );
    assert!(
        output.contains("function _asyncIterator("),
        "helper with a live call site was removed:\n{output}"
    );
    assert!(
        output.contains("return _asyncIterator(stream)"),
        "unrelated call site changed:\n{output}"
    );
}

#[test]
fn for_await_preserves_iterator_used_after_loop() {
    // `_iterator` escapes the protocol, so the loop must stay lowered.
    let input = with_helper(
        OLD_BABEL_ASYNC_ITERATOR,
        r#"
export async function consume(stream) {
  var _iteratorAbruptCompletion = false;
  var _didIteratorError = false;
  var _iteratorError;
  try {
    for (var _iterator = _asyncIterator(stream), _step; _iteratorAbruptCompletion = !(_step = await _iterator.next()).done; _iteratorAbruptCompletion = false) {
      const item = _step.value;
      await handle(item);
    }
  } catch (err) {
    _didIteratorError = true;
    _iteratorError = err;
  } finally {
    try {
      if (_iteratorAbruptCompletion && _iterator.return != null) {
        await _iterator.return();
      }
    } finally {
      if (_didIteratorError) {
        throw _iteratorError;
      }
    }
  }
  return _iterator;
}
"#,
    );
    let output = render(&input);
    assert!(
        !output.contains("for await"),
        "escaping iterator was folded into for await:\n{output}"
    );
    assert!(output.contains("_asyncIterator(stream)"), "{output}");
}

#[test]
fn for_await_preserves_unknown_adapter_callee() {
    // The protocol shape alone is not proof; the adapter must be a known
    // helper. A user function named like one keeps the loop.
    let input = r#"
function makeIterator(source) {
  return source.stream();
}
export async function consume(stream) {
  var _iteratorAbruptCompletion = false;
  var _didIteratorError = false;
  var _iteratorError;
  try {
    for (var _iterator = makeIterator(stream), _step; _iteratorAbruptCompletion = !(_step = await _iterator.next()).done; _iteratorAbruptCompletion = false) {
      const item = _step.value;
      await handle(item);
    }
  } catch (err) {
    _didIteratorError = true;
    _iteratorError = err;
  } finally {
    try {
      if (_iteratorAbruptCompletion && _iterator.return != null) {
        await _iterator.return();
      }
    } finally {
      if (_didIteratorError) {
        throw _iteratorError;
      }
    }
  }
}
"#;
    let output = render(input);
    assert!(!output.contains("for await"), "{output}");
    assert!(output.contains("makeIterator(stream)"), "{output}");
}

#[test]
fn for_await_preserves_flag_read_outside_protocol() {
    let input = with_helper(
        OLD_BABEL_ASYNC_ITERATOR,
        r#"
export async function consume(stream) {
  var _iteratorAbruptCompletion = false;
  var _didIteratorError = false;
  var _iteratorError;
  try {
    for (var _iterator = _asyncIterator(stream), _step; _iteratorAbruptCompletion = !(_step = await _iterator.next()).done; _iteratorAbruptCompletion = false) {
      const item = _step.value;
      await handle(item);
    }
  } catch (err) {
    _didIteratorError = true;
    _iteratorError = err;
  } finally {
    try {
      if (_iteratorAbruptCompletion && _iterator.return != null) {
        await _iterator.return();
      }
    } finally {
      if (_didIteratorError) {
        throw _iteratorError;
      }
    }
  }
  report(_didIteratorError);
}
"#,
    );
    let output = render(&input);
    assert!(!output.contains("for await"), "{output}");
}

#[test]
fn for_await_from_terser_mangled_esbuild_bundle() {
    // esbuild es2015 output through Terser compress+mangle: the helper names
    // are gone, the flags are merged into one `var`, and the async function
    // still has to be recovered from `__async` first.
    let input = r#"
var e=(e,r)=>(r=Symbol[e])?r:Symbol.for("Symbol."+e),r=(e,r,l)=>new Promise((t,n)=>{var o=e=>{try{i(l.next(e))}catch(e){n(e)}},a=e=>{try{i(l.throw(e))}catch(e){n(e)}},i=e=>e.done?t(e.value):Promise.resolve(e.value).then(o,a);i((l=l.apply(e,r)).next())}),l=(r,l,t)=>(l=r[e("asyncIterator")])?l.call(r):(r=r[e("iterator")](),l={},(t=(e,t)=>(t=r[e])&&(l[e]=e=>new Promise((l,n,o)=>(e=t.call(r,e),o=e.done,Promise.resolve(e.value).then(e=>l({value:e,done:o}),n)))))("next"),t("return"),l);export function t(e){return r(this,null,function*(){const r=[];try{try{for(var t=l(e),n,o,a;n=!(o=yield t.next()).done;n=!1){const e=o.value;if(e.done)break;r.push(yield normalize_item(e))}}catch(o){a=[o]}finally{try{n&&(o=t.return)&&(yield o.call(t))}finally{if(a)throw a[0]}}}finally{yield close_stream(e)}return r})}
"#;
    // The mangled element name `e` collides with the parameter the iterable
    // reads, so lifting it into the loop head would put the iterable in its
    // TDZ; the element is renamed inside the loop instead.
    let expected = r#"
export async function t(e) {
  const r = [];
  try {
    for await (const e_1 of e) {
      if (e_1.done) {
        break;
      }
      r.push(await normalize_item(e_1));
    }
  } finally {
    await close_stream(e);
  }
  return r;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_await_from_terser_compressed_old_babel_head() {
    // Babel 7.8–7.13 through Terser compress: the step assignment is folded
    // into the completion flag (`normal = (step = await it.next()).done`) and
    // the close guard is negated into `!(normal || it.return == null)`.
    let input = with_helper(
        OLD_BABEL_ASYNC_ITERATOR,
        r#"
async function consume_stream(stream){var _iteratorNormalCompletion=!0,_didIteratorError=!1,_iteratorError;try{for(var _iterator=_asyncIterator(stream),_step,_value;_iteratorNormalCompletion=(_step=await _iterator.next()).done,_value=await _step.value,!_iteratorNormalCompletion;_iteratorNormalCompletion=!0){const item=_value;await handle_item(item)}}catch(err){_didIteratorError=!0,_iteratorError=err}finally{try{_iteratorNormalCompletion||null==_iterator.return||(await _iterator.return())}finally{if(_didIteratorError)throw _iteratorError}}}
"#,
    );
    let expected = r#"
async function consume_stream(stream) {
  for await (const item of stream) {
    await handle_item(item);
  }
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_from_terser_mangled_babel_bundle_removes_helper_dependency() {
    // Babel 7.28 async-generator output through Terser compress+mangle. The
    // adapter is `t` and its `AsyncFromSyncIterator` dependency is `e`; both
    // must go once the loop is recovered, and the single-letter adapter must
    // not be mistaken for a tslib `__asyncValues` helper on the way.
    let input = r#"
function n(n,r,t,e,o,i,u){try{var l=n[i](u),a=l.value}catch(n){return void t(n)}l.done?r(a):Promise.resolve(a).then(e,o)}function r(r){return function(){var t=this,e=arguments;return new Promise(function(o,i){var u=r.apply(t,e);function l(r){n(u,o,i,l,a,"next",r)}function a(r){n(u,o,i,l,a,"throw",r)}l(void 0)})}}function t(n){var r,t,o,i=2;for("undefined"!=typeof Symbol&&(t=Symbol.asyncIterator,o=Symbol.iterator);i--;){if(t&&null!=(r=n[t]))return r.call(n);if(o&&null!=(r=n[o]))return new e(r.call(n));t="@@asyncIterator",o="@@iterator"}throw new TypeError("Object is not async iterable")}function e(n){function r(n){if(Object(n)!==n)return Promise.reject(new TypeError(n+" is not an object."));var r=n.done;return Promise.resolve(n.value).then(function(n){return{value:n,done:r}})}return e=function(n){this.s=n,this.n=n.next},e.prototype={s:null,n:null,next:function(){return r(this.n.apply(this.s,arguments))},return:function(n){var t=this.s.return;return void 0===t?Promise.resolve({value:n,done:!0}):r(t.apply(this.s,arguments))},throw:function(n){var t=this.s.return;return void 0===t?Promise.reject(n):r(t.apply(this.s,arguments))}},new e(n)}function o(n){return i.apply(this,arguments)}function i(){return(i=r(function*(n){const r=[];try{var e=!1,o=!1,i;try{for(var u=t(n),l;e=!(l=yield u.next()).done;e=!1){const n=l.value;if(n.done)break;r.push(yield normalize_item(n))}}catch(n){o=!0,i=n}finally{try{e&&null!=u.return&&(yield u.return())}finally{if(o)throw i}}}finally{yield close_stream(n)}return r})).apply(this,arguments)}
"#;
    let expected = r#"
async function o(n) {
  const r = [];
  try {
    for await (const n_1 of n) {
      if (n_1.done) {
        break;
      }
      r.push(await normalize_item(n_1));
    }
  } finally {
    await close_stream(n);
  }
  return r;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_await_binds_step_when_terser_inlined_the_element() {
    // Babel 7.8–7.13 through Terser compress: the single-use element
    // declaration is gone and the body reads the awaited value temporary
    // directly. The loop can only bind the protocol's step name.
    let input = with_helper(
        OLD_BABEL_ASYNC_ITERATOR,
        r#"
const total_size = async (files) => {
  let size = 0;
  var _iteratorNormalCompletion = true, _didIteratorError = false, _iteratorError;
  try {
    for (var _iterator = _asyncIterator(files), _step, _value; _iteratorNormalCompletion = (_step = await _iterator.next()).done, _value = await _step.value, !_iteratorNormalCompletion; _iteratorNormalCompletion = true) {
      size += _value.size;
    }
  } catch (err) {
    _didIteratorError = true, _iteratorError = err;
  } finally {
    try {
      _iteratorNormalCompletion || null == _iterator.return || (await _iterator.return());
    } finally {
      if (_didIteratorError) throw _iteratorError;
    }
  }
  return size;
};
use(total_size, total_size);
"#,
    );
    let expected = r#"
const total_size = async (files) => {
  let size = 0;
  for await (const _step of files) {
    size += _step.size;
  }
  return size;
};
use(total_size, total_size);
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_from_decoded_ts_machine_with_hoisted_temporaries() {
    // The shape a `__generator` decode hands to UnForOf: every temporary,
    // including the element, is hoisted to the function scope, the protocol
    // assigns them in place, and the catch boxes the error as `{ error }`.
    let input = with_helper(
        TS_ASYNC_VALUES,
        r#"
async function consume(stream) {
  let item;
  let _a;
  let stream_1;
  let stream_1_1;
  let _b;
  let e_1;
  let _c;
  let _d;
  try {
    _a = true;
    stream_1 = __asyncValues(stream);
    for (; stream_1_1 = await stream_1.next(), _b = stream_1_1.done, !_b; _a = true) {
      _d = stream_1_1.value;
      _a = false;
      item = _d;
      await handle(item);
    }
  } catch (error) {
    e_1 = { error };
  } finally {
    try {
      if (!_a && !_b && (_c = stream_1.return)) {
        await _c.call(stream_1);
      }
    } finally {
      if (e_1) {
        throw e_1.error;
      }
    }
  }
}
"#,
    );
    let expected = r#"
async function consume(stream) {
  for await (const item of stream) {
    await handle(item);
  }
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_removes_hoisted_temporaries_declared_outside_the_protocol_list() {
    // The protocol sits inside a user `try`, so its hoisted temporaries are
    // declared in the enclosing function body; the function body drops them
    // once the inner list has folded the loop.
    let input = with_helper(
        TS_ASYNC_VALUES,
        r#"
async function consume(stream) {
  let item;
  let _c;
  let stream_1;
  let stream_1_1;
  let _d;
  let e_1;
  let _e;
  let _f;
  const output = [];
  try {
    try {
      _c = true;
      stream_1 = __asyncValues(stream);
      for (; stream_1_1 = await stream_1.next(), _d = stream_1_1.done, !_d; _c = true) {
        _f = stream_1_1.value;
        _c = false;
        item = _f;
        if (item.done) {
          break;
        }
        output.push(await normalize(item));
      }
    } catch (error) {
      e_1 = { error };
    } finally {
      try {
        if (!_c && !_d && (_e = stream_1.return)) {
          await _e.call(stream_1);
        }
      } finally {
        if (e_1) {
          throw e_1.error;
        }
      }
    }
  } finally {
    await close(stream);
  }
  return output;
}
"#,
    );
    let expected = r#"
async function consume(stream) {
  const output = [];
  try {
    for await (const item of stream) {
      if (item.done) {
        break;
      }
      output.push(await normalize(item));
    }
  } finally {
    await close(stream);
  }
  return output;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_assigns_an_element_that_outlives_the_loop() {
    // `last` is read after the loop, so it stays declared outside and the loop
    // assigns it in place instead of declaring a per-iteration binding.
    let input = with_helper(
        TS_ASYNC_VALUES,
        r#"
async function tail(stream) {
  let last;
  let _a;
  let stream_1;
  let stream_1_1;
  let _b;
  let e_1;
  let _c;
  let _d;
  try {
    _a = true;
    stream_1 = __asyncValues(stream);
    for (; stream_1_1 = await stream_1.next(), _b = stream_1_1.done, !_b; _a = true) {
      _d = stream_1_1.value;
      _a = false;
      last = _d;
      await handle(last);
    }
  } catch (error) {
    e_1 = { error };
  } finally {
    try {
      if (!_a && !_b && (_c = stream_1.return)) {
        await _c.call(stream_1);
      }
    } finally {
      if (e_1) {
        throw e_1.error;
      }
    }
  }
  return last;
}
"#,
    );
    let expected = r#"
async function tail(stream) {
  let last;
  for await (last of stream) {
    await handle(last);
  }
  return last;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_folds_an_element_alias_inlined_into_the_first_statement() {
    // Terser compress on the TypeScript machine: `item = _d` survives only as
    // an assignment expression inside the first body statement.
    let input = with_helper(
        TS_ASYNC_VALUES,
        r#"
async function consume(stream) {
  let item;
  let _a;
  let stream_1;
  let stream_1_1;
  let _b;
  let e_1;
  let _c;
  let _d;
  try {
    _a = true;
    stream_1 = __asyncValues(stream);
    for (; stream_1_1 = await stream_1.next(), _b = stream_1_1.done, !_b; _a = true) {
      _d = stream_1_1.value;
      _a = false;
      if ((item = _d).done) break;
      await handle(item);
    }
  } catch (error) {
    e_1 = { error };
  } finally {
    try {
      if (!(_a || _b || !(_c = stream_1.return))) {
        await _c.call(stream_1);
      }
    } finally {
      if (e_1) {
        throw e_1.error;
      }
    }
  }
}
"#,
    );
    let expected = r#"
async function consume(stream) {
  for await (const item of stream) {
    if (item.done) {
      break;
    }
    await handle(item);
  }
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_folds_an_inlined_alias_read_later_in_the_same_statement() {
    // The alias assignment sits inside the guard's awaited call and the
    // consequent reads the alias afterwards; only a read before the
    // assignment would make the fold unsound.
    let input = with_helper(
        TS_ASYNC_VALUES,
        r#"
async function find(stream, predicate) {
  let entry;
  let _a;
  let stream_1;
  let stream_1_1;
  let _b;
  let e_1;
  let _c;
  let _d;
  try {
    _a = true;
    stream_1 = __asyncValues(stream);
    for (; stream_1_1 = await stream_1.next(), _b = stream_1_1.done, !_b; _a = true) {
      _d = stream_1_1.value;
      _a = false;
      if (await predicate(entry = _d)) return entry;
    }
  } catch (error) {
    e_1 = { error };
  } finally {
    try {
      if (!(_a || _b || !(_c = stream_1.return))) {
        await _c.call(stream_1);
      }
    } finally {
      if (e_1) {
        throw e_1.error;
      }
    }
  }
  return null;
}
"#,
    );
    let expected = r#"
async function find(stream, predicate) {
  for await (const entry of stream) {
    if (await predicate(entry)) {
      return entry;
    }
  }
  return null;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_keeps_an_inlined_alias_that_is_read_before_it_is_assigned() {
    // `entry` is read in the index before the assignment writes it, so the
    // previous iteration's value is observable; the alias must stay.
    let input = with_helper(
        TS_ASYNC_VALUES,
        r#"
async function walk(stream, table) {
  let entry;
  let _a;
  let stream_1;
  let stream_1_1;
  let _b;
  let e_1;
  let _c;
  let _d;
  try {
    _a = true;
    stream_1 = __asyncValues(stream);
    for (; stream_1_1 = await stream_1.next(), _b = stream_1_1.done, !_b; _a = true) {
      _d = stream_1_1.value;
      _a = false;
      table[entry] = entry = _d;
    }
  } catch (error) {
    e_1 = { error };
  } finally {
    try {
      if (!(_a || _b || !(_c = stream_1.return))) {
        await _c.call(stream_1);
      }
    } finally {
      if (e_1) {
        throw e_1.error;
      }
    }
  }
}
"#,
    );
    let output = render(&input);
    assert!(output.contains("table[entry] = entry ="), "{output}");
}

#[test]
fn for_await_keeps_a_protocol_iterator_read_in_the_body() {
    // The body calls the iterator's `return()` itself, so the iterator
    // temporary is not the protocol's alone and its declaration must stay.
    let input = with_helper(
        OLD_BABEL_ASYNC_ITERATOR,
        r#"
export async function consume(stream) {
  var _iteratorAbruptCompletion = false;
  var _didIteratorError = false;
  var _iteratorError;
  try {
    for (var _iterator = _asyncIterator(stream), _step; _iteratorAbruptCompletion = !(_step = await _iterator.next()).done; _iteratorAbruptCompletion = false) {
      const item = _step.value;
      if (item.stop) {
        await _iterator.return();
      }
      await handle(item);
    }
  } catch (err) {
    _didIteratorError = true;
    _iteratorError = err;
  } finally {
    try {
      if (_iteratorAbruptCompletion && _iterator.return != null) {
        await _iterator.return();
      }
    } finally {
      if (_didIteratorError) {
        throw _iteratorError;
      }
    }
  }
}
"#,
    );
    let output = render(&input);
    assert!(!output.contains("for await"), "{output}");
    assert!(
        output.contains("_iterator = _asyncIterator(stream)"),
        "{output}"
    );
}

#[test]
fn for_await_keeps_a_flag_declared_in_the_try_that_the_body_reads() {
    // A boolean-initialized declaration ahead of the loop looks like a
    // protocol flag, but the body reads and writes it; consuming its
    // declaration would leave the body referencing an undeclared name.
    let input = with_helper(
        OLD_BABEL_ASYNC_ITERATOR,
        r#"
export async function consume(stream) {
  var _iteratorAbruptCompletion = false;
  var _didIteratorError = false;
  var _iteratorError;
  try {
    var first = true;
    for (var _iterator = _asyncIterator(stream), _step; _iteratorAbruptCompletion = !(_step = await _iterator.next()).done; _iteratorAbruptCompletion = false) {
      const item = _step.value;
      if (first) {
        first = false;
        onFirst(item);
      } else {
        onRest(item);
      }
    }
  } catch (err) {
    _didIteratorError = true;
    _iteratorError = err;
  } finally {
    try {
      if (_iteratorAbruptCompletion && _iterator.return != null) {
        await _iterator.return();
      }
    } finally {
      if (_didIteratorError) {
        throw _iteratorError;
      }
    }
  }
}
"#,
    );
    let output = render(&input);
    assert!(!output.contains("for await"), "{output}");
    assert!(output.contains("first = true"), "{output}");
}

#[test]
fn for_await_keeps_an_uninitialized_temp_declared_in_the_try_that_the_body_writes() {
    let input = with_helper(
        TS_ASYNC_VALUES,
        r#"
export async function consume(stream) {
  var e_1, _a;
  try {
    var count;
    for (var stream_1 = __asyncValues(stream), stream_1_1; stream_1_1 = await stream_1.next(), !stream_1_1.done;) {
      const item = stream_1_1.value;
      count = (count || 0) + 1;
      await handle(item, count);
    }
  } catch (e_1_1) {
    e_1 = { error: e_1_1 };
  } finally {
    try {
      if (stream_1_1 && !stream_1_1.done && (_a = stream_1.return)) await _a.call(stream_1);
    } finally {
      if (e_1) throw e_1.error;
    }
  }
}
"#,
    );
    let output = render(&input);
    assert!(!output.contains("for await"), "{output}");
    assert!(
        output.contains("let count;") || output.contains("var count;"),
        "{output}"
    );
}

#[test]
fn for_await_assigns_a_hoisted_element_captured_by_a_closure() {
    // Every lexical use of `item` sits inside the protocol, but the closures
    // read it after the loop: a per-iteration `const` would give each thunk
    // its own element where the hoisted binding gives them all the last one.
    let input = with_helper(
        TS_ASYNC_VALUES,
        r#"
export async function collect(stream) {
  let item;
  let _a;
  let stream_1;
  let stream_1_1;
  let _b;
  let e_1;
  let _c;
  let _d;
  const thunks = [];
  try {
    _a = true;
    stream_1 = __asyncValues(stream);
    for (; stream_1_1 = await stream_1.next(), _b = stream_1_1.done, !_b; _a = true) {
      _d = stream_1_1.value;
      _a = false;
      item = _d;
      thunks.push(() => item);
    }
  } catch (error) {
    e_1 = { error };
  } finally {
    try {
      if (!_a && !_b && (_c = stream_1.return)) {
        await _c.call(stream_1);
      }
    } finally {
      if (e_1) {
        throw e_1.error;
      }
    }
  }
  return thunks;
}
"#,
    );
    let expected = r#"
export async function collect(stream) {
  let item;
  const thunks = [];
  for await (item of stream) {
    thunks.push(() => item);
  }
  return thunks;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn for_await_keeps_an_inlined_alias_assigned_inside_a_nested_function() {
    // The alias assignment runs when the thunk is called, not when the
    // statement executes, so it is not the element binding of this iteration.
    let input = with_helper(
        TS_ASYNC_VALUES,
        r#"
export async function collect(stream) {
  let item;
  let _a;
  let stream_1;
  let stream_1_1;
  let _b;
  let e_1;
  let _c;
  let _d;
  const thunks = [];
  try {
    _a = true;
    stream_1 = __asyncValues(stream);
    for (; stream_1_1 = await stream_1.next(), _b = stream_1_1.done, !_b; _a = true) {
      _d = stream_1_1.value;
      _a = false;
      thunks.push(() => (item = _d));
    }
  } catch (error) {
    e_1 = { error };
  } finally {
    try {
      if (!_a && !_b && (_c = stream_1.return)) {
        await _c.call(stream_1);
      }
    } finally {
      if (e_1) {
        throw e_1.error;
      }
    }
  }
  return thunks;
}
"#,
    );
    let expected = r#"
export async function collect(stream) {
  let item;
  let _d;
  const thunks = [];
  for await (_d of stream) {
    thunks.push(() => item = _d);
  }
  return thunks;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}
