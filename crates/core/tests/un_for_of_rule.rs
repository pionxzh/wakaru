mod common;

use common::{assert_eq_normalized, render, render_pipeline_until, render_rule};
use wakaru_core::facts::{
    ModuleFacts, ModuleFactsMap, TypeScriptHelperExportFact, TypeScriptHelperKind,
};
use wakaru_core::{
    decompile, rules::UnForOf, validate_output_modules, DecompileOptions, OutputFindingKind,
    RewriteLevel,
};

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |mark| UnForOf::new_with_mark(mark, level))
}

#[test]
fn for_of_from_closure_make_iterator() {
    // Produced by Closure Compiler v20260629 with SIMPLE optimizations and
    // language_out=ECMASCRIPT5. The compiler reuses the parameter as its
    // iterator temporary when the original iterable does not escape.
    let input = r#"
function total(items) {
  var sum = 0;
  items = $jscomp.makeIterator(items);
  for (var step = items.next(); !step.done; step = items.next()) {
    sum += step.value;
  }
  return sum;
}
"#;
    let expected = r#"
function total(items) {
  var sum = 0;
  for (const step of items) {
    sum += step;
  }
  return sum;
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn for_of_from_closure_make_iterator_in_loop_initializer() {
    // Closure keeps a distinct iterator when the original iterable is used
    // after the loop.
    let input = r#"
function count(items) {
  for (var iterator = $jscomp.makeIterator(items), step = iterator.next(); !step.done; step = iterator.next()) {
    use(step.value);
  }
  return items.length;
}
"#;
    let expected = r#"
function count(items) {
  for (const step of items) {
    use(step);
  }
  return items.length;
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn minimal_preserves_closure_make_iterator() {
    let input = r#"
function total(items) {
  items = $jscomp.makeIterator(items);
  for (var step = items.next(); !step.done; step = items.next()) {
    use(step.value);
  }
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn for_of_from_locally_bootstrapped_closure_namespace() {
    let input = r#"
var $jscomp = $jscomp || {};
$jscomp.makeIterator = function(value) { return makeIterator(value); };
function total(items) {
  var sum = 0;
  items = $jscomp.makeIterator(items);
  for (var step = items.next(); !step.done; step = items.next()) {
    sum += step.value;
  }
  return sum;
}
"#;
    let expected = r#"
var $jscomp = $jscomp || {};
$jscomp.makeIterator = function(value) { return makeIterator(value); };
function total(items) {
  var sum = 0;
  for (const step of items) {
    sum += step;
  }
  return sum;
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn closure_make_iterator_requires_namespace_provenance() {
    let input = r#"
function total($jscomp, items) {
  var sum = 0;
  items = $jscomp.makeIterator(items);
  for (var step = items.next(); !step.done; step = items.next()) {
    sum += step.value;
  }
  return sum;
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Standard), input);
}

#[test]
fn closure_make_iterator_preserves_escaping_iterator_binding() {
    let input = r#"
function total(items) {
  var sum = 0;
  items = $jscomp.makeIterator(items);
  for (var step = items.next(); !step.done; step = items.next()) {
    sum += step.value;
  }
  consumeIterator(items);
  return sum;
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Standard), input);
}

#[test]
fn closure_make_iterator_preserves_iterator_escaping_enclosing_block() {
    let input = r#"
var $jscomp = $jscomp || {};
function demo(flag, items) {
  if (flag) {
    var iterator = $jscomp.makeIterator(items);
    for (var step = iterator.next(); !step.done; step = iterator.next()) {
      consume(step.value);
    }
  }
  return iterator;
}
"#;
    let before = render_pipeline_until(input, "ArrowReturn");
    let after = render_pipeline_until(input, "UnForOf");
    assert_eq_normalized(&after, &before);
}

#[test]
fn basic_for_to_for_of() {
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { const x = arr[i]; console.log(x); }"#;
    let expected = r#"for (const x of items) { console.log(x); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn minimal_does_not_convert_basic_for_to_for_of() {
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { const x = arr[i]; console.log(x); }"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn for_of_with_block_body() {
    let input = r#"for (let Y = 0, V = list; Y < V.length; Y++) { const Z = V[Y]; if (Z != null) { process(Z); } }"#;
    let expected = r#"for (const Z of list) { if (Z != null) { process(Z); } }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_with_method_call_iterable() {
    let input =
        r#"for (let Y = 0, V = Object.keys(obj); Y < V.length; Y++) { const Z = V[Y]; use(Z); }"#;
    let expected = r#"for (const Z of Object.keys(obj)) { use(Z); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn no_transform_when_index_used_in_body() {
    // Index `i` is used beyond just arr[i], so can't convert
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { const x = arr[i]; console.log(i, x); }"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn no_transform_when_arr_used_in_body() {
    // arr variable used beyond arr[i] and arr.length
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { const x = arr[i]; console.log(arr.length, x); }"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn no_transform_when_no_elem_decl() {
    // No `const elem = arr[i]` as first statement
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { console.log(arr[i]); }"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn no_transform_regular_for_loop() {
    let input = r#"for (let i = 0; i < 10; i++) { console.log(i); }"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn for_of_uses_let_when_elem_reassigned() {
    // P3 regression: elem is reassigned so for-of must use `let`, not `const`
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { let elem = arr[i]; elem = normalize(elem); process(elem); }"#;
    let expected = r#"for (let elem of items) { elem = normalize(elem); process(elem); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_nested_write_keeps_computed_key_assignable() {
    let input = r#"
for (let i = 0, keys = Object.keys(input); i < keys.length; i++) {
    let key = keys[i];
    out[key = key.toUpperCase()] = input[key];
}
"#;
    let expected = r#"
for (let key of Object.keys(input)) {
    out[key = key.toUpperCase()] = input[key];
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn for_of_nested_write_inventory_for_indexed_and_helper_loops() {
    // Both recovery paths must inspect every write position in the remaining
    // body, including writes underneath another assignment or update target.
    let writes = [
        "out[item = next()] = value;",
        "out.value = item = next();",
        "out[item++] = value;",
        "out[item = next()]++;",
        "[item] = values;",
        "({ value: item } = source);",
        "[other = item = next()] = values;",
        "item ||= next();",
        "(item)++;",
        "for (item of values) { use(item); }",
        "for (item in source) { use(item); }",
        "save(() => { item = next(); });",
        "use({ [item = next()]: value });",
    ];
    for body in writes {
        let indexed = format!(
            "for (let i = 0, arr = items; i < arr.length; i++) {{ let item = arr[i]; {body} }}"
        );
        let helper = format!(
            "let step; for (const iterator = _createForOfIteratorHelperLoose(items); !(step = iterator()).done;) {{ let item = step.value; {body} }}"
        );
        let expected_body = body.replace("(item)++", "item++");
        let expected = format!("for (let item of items) {{ {expected_body} }}");
        for input in [indexed, helper] {
            assert_eq_normalized(&apply_with_level(&input, RewriteLevel::Standard), &expected);
        }
    }
}

#[test]
fn for_of_nested_write_analysis_excludes_other_bindings_and_properties() {
    for body in [
        "item.value = next();",
        "item[index]++;",
        "{ let item; out[item = next()] = value; }",
        "save((item) => { item = next(); });",
    ] {
        let indexed = format!(
            "for (let i = 0, arr = items; i < arr.length; i++) {{ let item = arr[i]; {body} }}"
        );
        let helper = format!(
            "let step; for (const iterator = _createForOfIteratorHelperLoose(items); !(step = iterator()).done;) {{ let item = step.value; {body} }}"
        );
        let expected = format!("for (const item of items) {{ {body} }}");
        for input in [indexed, helper] {
            assert_eq_normalized(&apply_with_level(&input, RewriteLevel::Standard), &expected);
        }
    }
}

#[test]
fn for_of_nested_write_keeps_destructured_binding_assignable() {
    let input = r#"
for (let i = 0; i < entries.length; i++) {
    let pair = entries[i];
    let key = pair[0];
    let value = pair[1];
    out[key = normalize(key)] = value;
}
"#;
    let expected = r#"
for (let [key, value] of entries) {
    out[key = normalize(key)] = value;
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn for_of_preserves_writes_to_original_const_bindings() {
    for (indexed_decls, helper_decls, body) in [
        (
            "const item = arr[i];",
            "const item = step.value;",
            "out[item = next()] = value;",
        ),
        (
            "const item = arr[i];",
            "const item = step.value;",
            "item = next();",
        ),
        (
            "const pair = arr[i]; const key = pair[0]; let value = pair[1];",
            "const pair = step.value; const key = pair[0]; let value = pair[1];",
            "out[key = next()] = value;",
        ),
    ] {
        let indexed = format!(
            "for (let i = 0, arr = items; i < arr.length; i++) {{ {indexed_decls} {body} }}"
        );
        let helper = format!(
            "let step; for (const iterator = _createForOfIteratorHelperLoose(items); !(step = iterator()).done;) {{ {helper_decls} {body} }}"
        );
        for input in [indexed, helper] {
            assert_eq_normalized(&apply_with_level(&input, RewriteLevel::Standard), &input);
        }
    }
}

#[test]
fn for_of_single_decl_arr_form() {
    let input =
        r#"for (let Y = 0, V = B.split("."); Y < V.length; Y++) { const Z = V[Y]; process(Z); }"#;
    let expected = r#"for (const Z of B.split(".")) { process(Z); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_direct_array_index_form() {
    // Babel with the `iterableIsArray` assumption emits direct indexed loops.
    let input = r#"for (let i = 0; i < items.length; i++) { const item = items[i]; use(item); }"#;
    let expected = r#"for (const item of items) { use(item); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_direct_array_index_uses_let_when_elem_reassigned() {
    let input = r#"for (let i = 0; i < items.length; i++) { let item = items[i]; item = normalize(item); use(item); }"#;
    let expected = r#"for (let item of items) { item = normalize(item); use(item); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn indexed_loop_keeps_var_index_used_by_a_later_loop() {
    let input = r#"
const index = -1;
function replay(items) {
  const seen = [];
  for (var index = 0; index < items.length; index++) {
    var item = items[index];
    seen.push(item);
  }
  for (index = 0; index < items.length; index++) {
    seen.push(items[index]);
  }
  return seen;
}
"#;

    let after_rule = apply_with_level(input, RewriteLevel::Standard);
    assert_eq_normalized(&after_rule, input);

    let output = render(input);
    let findings = validate_output_modules(&[("input.js".to_string(), output.clone())]);
    assert!(
        findings.is_empty(),
        "the later loop must retain the function-scoped index binding:\n{output}\n{findings:#?}"
    );
}

#[test]
fn preserves_legacy_call_target_for_of_over_empty_array() {
    let input = r#"
for (observe("never") of []);
keepRunning();
"#;
    let output = render(input);

    assert!(
        output.contains("observe"),
        "authored legacy syntax must remain visible: {output}"
    );
    assert!(
        output.contains("keepRunning"),
        "neighboring statements must remain: {output}"
    );
    let findings = validate_output_modules(&[("input.js".to_string(), output.clone())]);
    assert_eq!(
        findings.len(),
        1,
        "the preserved engine-specific syntax should remain visible to validation: {output}\n{findings:#?}"
    );
    assert_eq!(findings[0].kind, OutputFindingKind::ParseError);
    assert!(
        findings[0].message.contains("TS2406"),
        "SWC should report the non-assignable call target: {findings:#?}"
    );
}

#[test]
fn legacy_empty_array_loop_body_is_not_rewritten() {
    let input = r#"
function f() {
    for (g() of []) {
        function helper() { return 1; }
        var { a, b: [c] } = source();
    }
    use(helper, a, c);
}
"#;

    for level in [RewriteLevel::Standard, RewriteLevel::Aggressive] {
        assert_eq_normalized(&apply_with_level(input, level), input);
    }
}

#[test]
fn indexed_loop_keeps_iterable_temp_used_after_the_loop() {
    let input = r#"
function replay(items) {
  for (var index = 0, values = items; index < values.length; index++) {
    var item = values[index];
    use(item);
  }
  return values;
}
"#;

    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Standard), input);
}

#[test]
fn for_of_preserves_var_when_var_decl_survives() {
    let input = r#"
function f(items) {
  var item = fallback;
  for (let i = 0; i < items.length; i++) {
    var item = items[i];
    use(item);
  }
  return item;
}
"#;
    let expected = r#"
function f(items) {
  var item = fallback;
  for (var item of items) {
    use(item);
  }
  return item;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_destructuring_preserves_mixed_var_binding_kind() {
    let input = r#"
function recover(rows) {
  var key = 0;
  consume(key);
  for (var index = 0; index < rows.length; index++) {
    var pair = rows[index], key = pair[0];
  }
}
"#;
    let expected = r#"
function recover(rows) {
  var key = 0;
  consume(key);
  for (var [key] of rows) {}
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_destructuring_from_ts_index_form() {
    let input = r#"for (let i = 0, entries_1 = entries; i < entries_1.length; i++) { const _a = entries_1[i], key = _a[0], value = _a[1]; use(key, value); }"#;
    let expected = r#"for (const [key, value] of entries) { use(key, value); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_destructuring_from_direct_array_index_form() {
    let input = r#"for (let i = 0; i < entries.length; i++) { const _entry = entries[i], key = _entry[0], value = _entry[1]; use(key, value); }"#;
    let expected = r#"for (const [key, value] of entries) { use(key, value); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_destructuring_uses_let_when_binding_reassigned() {
    let input = r#"for (let i = 0; i < entries.length; i++) { let _entry = entries[i], key = _entry[0], value = _entry[1]; key = normalize(key); use(key, value); }"#;
    let expected =
        r#"for (let [key, value] of entries) { key = normalize(key); use(key, value); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn no_transform_destructuring_when_temp_used_later() {
    let input = r#"for (let i = 0; i < entries.length; i++) { const _entry = entries[i], key = _entry[0], value = _entry[1]; use(_entry, key, value); }"#;
    let expected = r#"for (let i = 0; i < entries.length; i++) { const _entry = entries[i]; const key = _entry[0]; const value = _entry[1]; use(_entry, key, value); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_from_babel_iterator_helper() {
    let input = r#"
let step;
const iterator = _createForOfIteratorHelper(items);
try {
  for (iterator.s(); !(step = iterator.n()).done;) {
    const item = step.value;
    use(item);
  }
} catch (err) {
  iterator.e(err);
} finally {
  iterator.f();
}
"#;
    let expected = r#"
for (const item of items) {
  use(item);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_from_babel_iterator_helper_rewrites_value_refs() {
    let input = r#"
let step;
let last;
const iterator = _createForOfIteratorHelper(items);
try {
  for (iterator.s(); !(step = iterator.n()).done;) {
    last = step.value;
  }
} catch (err) {
  iterator.e(err);
} finally {
  iterator.f();
}
return last;
"#;
    let expected = r#"
let last;
for (const step of items) {
  last = step;
}
return last;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_from_babel_iterator_helper_decl_first() {
    let input = r#"
const iterator = _createForOfIteratorHelper(items);
let step;
try {
  for (iterator.s(); !(step = iterator.n()).done;) {
    const item = step.value;
    use(item);
  }
} catch (err) {
  iterator.e(err);
} finally {
  iterator.f();
}
"#;
    let expected = r#"
for (const item of items) {
  use(item);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_from_babel_loose_iterator_helper() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(items); !(step = iterator()).done;) {
  const item = step.value;
  use(item);
}
"#;
    let expected = r#"
for (const item of items) {
  use(item);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_destructuring_from_iterator_helper() {
    let input = r#"
const iterator = _createForOfIteratorHelper(entries);
let step;
try {
  for (iterator.s(); !(step = iterator.n()).done;) {
    const pair = step.value;
    const key = pair[0];
    const value = pair[1];
    use(key, value);
  }
} catch (err) {
  iterator.e(err);
} finally {
  iterator.f();
}
"#;
    let expected = r#"
for (const [key, value] of entries) {
  use(key, value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn iterator_value_destructuring_preserves_mixed_var_binding_kind() {
    let input = r#"
function recover(entries) {
  var key = 0;
  consume(key);
  const iterator = _createForOfIteratorHelper(entries);
  let step;
  try {
    for (iterator.s(); !(step = iterator.n()).done;) {
      const pair = step.value;
      var key = pair[0];
    }
  } catch (err) {
    iterator.e(err);
  } finally {
    iterator.f();
  }
}
"#;
    let expected = r#"
function recover(entries) {
  var key = 0;
  consume(key);
  for (var [key] of entries) {}
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_destructuring_from_iterator_helper_read_call() {
    let input = r#"
const iterator = _createForOfIteratorHelper(entries);
let step;
try {
  for (iterator.s(); !(step = iterator.n()).done;) {
    const pair = _slicedToArray(step.value, 2);
    const key = pair[0];
    const value = pair[1];
    use(key, value);
  }
} catch (err) {
  iterator.e(err);
} finally {
  iterator.f();
}
"#;
    let expected = r#"
for (const [key, value] of entries) {
  use(key, value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn iterator_call_destructuring_preserves_mixed_var_binding_kind() {
    let input = r#"
function recover(entries) {
  var key = 0;
  consume(key);
  const iterator = _createForOfIteratorHelper(entries);
  let step;
  try {
    for (iterator.s(); !(step = iterator.n()).done;) {
      const pair = _slicedToArray(step.value, 1);
      var key = pair[0];
    }
  } catch (err) {
    iterator.e(err);
  } finally {
    iterator.f();
  }
}
"#;
    let expected = r#"
function recover(entries) {
  var key = 0;
  consume(key);
  for (var [key] of entries) {}
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_promotes_body_destructuring_from_iterator_value() {
    let input = r#"
const iterator = _createForOfIteratorHelper(entries);
let step;
try {
  for (iterator.s(); !(step = iterator.n()).done;) {
    const [key, value] = step.value;
    use(key, value);
  }
} catch (err) {
  iterator.e(err);
} finally {
  iterator.f();
}
"#;
    let expected = r#"
for (const [key, value] of entries) {
  use(key, value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_preserves_destructuring_from_iterator_result() {
    let input = r#"
const iterator = _createForOfIteratorHelper(entries);
let step;
try {
  for (iterator.s(); !(step = iterator.n()).done;) {
    const [key, value] = step;
    use(key, value);
  }
} catch (err) {
  iterator.e(err);
} finally {
  iterator.f();
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn for_of_preserves_destructuring_helper_from_iterator_result() {
    let input = r#"
const iterator = _createForOfIteratorHelper(entries);
let step;
try {
  for (iterator.s(); !(step = iterator.n()).done;) {
    const pair = _slicedToArray(step, 2);
    const key = pair[0];
    const value = pair[1];
    use(key, value);
  }
} catch (err) {
  iterator.e(err);
} finally {
  iterator.f();
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn for_of_from_ts_values_helper() {
    let input = r#"
var tslib = require("tslib");
let errorState;
let iteratorReturn;
try {
  for (var iterator = tslib.__values(items), step = iterator.next(); !step.done; step = iterator.next()) {
    const item = step.value;
    use(item);
  }
} catch (error) {
  errorState = { error };
} finally {
  try {
    if (step && !step.done && (iteratorReturn = iterator.return)) {
      iteratorReturn.call(iterator);
    }
  } finally {
    if (errorState) {
      throw errorState.error;
    }
  }
}
"#;
    let expected = r#"
import tslib from "tslib";
for (const item of items) {
  use(item);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_consumed_mangled_inline_ts_values_helper() {
    // Produced by TypeScript ES5 downlevelIteration, then Terser compress+mangle.
    let input = r#"
var r=this&&this.__values||function(r){var e="function"==typeof Symbol&&Symbol.iterator,t=e&&r[e],n=0;if(t)return t.call(r);if(r&&"number"==typeof r.length)return{next:function(){return r&&n>=r.length&&(r=void 0),{value:r&&r[n++],done:!r}}};throw new TypeError(e?"Object is not iterable.":"Symbol.iterator is not defined.")};export function f(e){var t,n;try{for(var o=r(e),i=o.next();!i.done;i=o.next()){var l=i.value;use(l)}}catch(r){t={error:r}}finally{try{i&&!i.done&&(n=o.return)&&n.call(o)}finally{if(t)throw t.error}}}
"#;
    let expected = r#"
export function f(e) {
  for (const l of e) {
    use(l);
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_preserves_unproven_ts_values_member() {
    let input = r#"
function run(tslib, items) {
  let errorState;
  let iteratorReturn;
  try {
    for (var iterator = tslib.__values(items), step = iterator.next(); !step.done; step = iterator.next()) {
      const item = step.value;
      use(item);
    }
  } catch (error) {
    errorState = { error };
  } finally {
    try {
      if (step && !step.done && (iteratorReturn = iterator.return)) {
        iteratorReturn.call(iterator);
      }
    } finally {
      if (errorState) {
        throw errorState.error;
      }
    }
  }
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Standard), input);
}

#[test]
fn for_of_preserves_unproven_ts_values_ident() {
    let input = r#"
function run(__values, items) {
  let errorState;
  let iteratorReturn;
  try {
    for (var iterator = __values(items), step = iterator.next(); !step.done; step = iterator.next()) {
      const item = step.value;
      use(item);
    }
  } catch (error) {
    errorState = { error };
  } finally {
    try {
      if (step && !step.done && (iteratorReturn = iterator.return)) {
        iteratorReturn.call(iterator);
      }
    } finally {
      if (errorState) {
        throw errorState.error;
      }
    }
  }
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Standard), input);
}

#[test]
fn for_of_from_cross_module_values_namespace_factory() {
    let input = r#"
import { tslibModule } from "./tslib-module.js";
const tslib = tslibModule();
let errorState;
let iteratorReturn;
try {
  for (var iterator = tslib.__values(items), step = iterator.next(); !step.done; step = iterator.next()) {
    const item = step.value;
    use(item);
  }
} catch (error) {
  errorState = { error };
} finally {
  try {
    if (step && !step.done && (iteratorReturn = iterator.return)) {
      iteratorReturn.call(iterator);
    }
  } finally {
    if (errorState) {
      throw errorState.error;
    }
  }
}
"#;
    let expected = r#"
import { tslibModule } from "./tslib-module.js";
const tslib = tslibModule();
for (const item of items) {
  use(item);
}
"#;

    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "./tslib-module.js",
        ModuleFacts {
            ts_helper_exports: vec![TypeScriptHelperExportFact {
                exported: "__values".into(),
                local: Some("values".into()),
                kind: TypeScriptHelperKind::Values,
            }],
            ..Default::default()
        },
    );

    let output = render_rule(input, |mark| {
        UnForOf::new_with_mark_and_facts(mark, RewriteLevel::Standard, &facts)
    });
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_of_from_nested_cross_module_values_namespace_factory() {
    let input = r#"
import { tslibModule } from "./tslib-module.js";
export function run(items) {
  const tslib = tslibModule();
  let errorState;
  let iteratorReturn;
  try {
    for (var iterator = tslib.__values(items), step = iterator.next(); !step.done; step = iterator.next()) {
      const item = step.value;
      use(item);
    }
  } catch (error) {
    errorState = { error };
  } finally {
    try {
      if (step && !step.done && (iteratorReturn = iterator.return)) {
        iteratorReturn.call(iterator);
      }
    } finally {
      if (errorState) {
        throw errorState.error;
      }
    }
  }
}
"#;
    let expected = r#"
import { tslibModule } from "./tslib-module.js";
export function run(items) {
  const tslib = tslibModule();
  for (const item of items) {
    use(item);
  }
}
"#;

    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "./tslib-module.js",
        ModuleFacts {
            ts_helper_exports: vec![TypeScriptHelperExportFact {
                exported: "__values".into(),
                local: Some("values".into()),
                kind: TypeScriptHelperKind::Values,
            }],
            ..Default::default()
        },
    );

    let output = render_rule(input, |mark| {
        UnForOf::new_with_mark_and_facts(mark, RewriteLevel::Standard, &facts)
    });
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_of_from_swc_symbol_iterator_helper() {
    let input = r#"
let normal = true;
let didError = false;
let iteratorError;
try {
  let step;
  for (var iterator = items[Symbol.iterator](); !(normal = (step = iterator.next()).done); normal = true) {
    const item = step.value;
    use(item);
  }
} catch (err) {
  didError = true;
  iteratorError = err;
} finally {
  try {
    if (!normal && iterator.return != null) {
      iterator.return();
    }
  } finally {
    if (didError) {
      throw iteratorError;
    }
  }
}
"#;
    let expected = r#"
for (const item of items) {
  use(item);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_from_swc_symbol_iterator_helper_merged_header() {
    // The raw swc-es5 (+ terser) shape: completion flags merged into one `var`
    // and the uninitialized `step` still inside the for header. UnVariableMerging
    // must pull `step` out (a no-init declarator is always safe to extract) or
    // the UnForOf matcher never fires. Regression test for the for-of matrix
    // drop to 73.9% (see un_variable_merging_rule.rs).
    let input = r#"
export function f(items) {
  var normal = true, didError = false, iteratorError = void 0;
  try {
    for (var iterator = items[Symbol.iterator](), step; !(normal = (step = iterator.next()).done); normal = true) {
      var item = step.value;
      use(item);
    }
  } catch (err) {
    didError = true;
    iteratorError = err;
  } finally {
    try {
      normal || null == iterator.return || iterator.return();
    } finally {
      if (didError) throw iteratorError;
    }
  }
}
"#;
    let expected = r#"
export function f(items) {
  for (const item of items) {
    use(item);
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn shadowed_binding_does_not_force_let() {
    let input = r#"
for (var i = 0; i < items.length; i++) {
    var item = items[i];
    console.log(item);
    {
        let item = transform();
        item = item + 1;
    }
}
"#;
    let expected = r#"
for (const item of items) {
  console.log(item);
  {
    let item = transform();
    item = item + 1;
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn recovers_for_of_from_imported_iterator_helper() {
    // The for-of loop is recovered by shape matching. The helper import
    // becomes dead — DeadImports removes it when DceMode is enabled.
    let input = r#"
import _createForOfIteratorHelper from "@babel/runtime/helpers/createForOfIteratorHelper";
let step;
const iterator = _createForOfIteratorHelper(items);
try {
  for (iterator.s(); !(step = iterator.n()).done;) {
    const item = step.value;
    use(item);
  }
} catch (err) {
  iterator.e(err);
} finally {
  iterator.f();
}
"#;
    let expected = r#"
import _createForOfIteratorHelper from "@babel/runtime/helpers/createForOfIteratorHelper";
for (const item of items) {
  use(item);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

// Babel loose Map/array entries: unused key DCE leaves `pair[0];` as an
// expression statement. Recover a hole, never invent a binding name.
// leftover of for-init sequence order (#224); does not skip intervening
// statements between `let step` and the helper `for`.

#[test]
fn for_of_from_babel_loose_discards_unused_index_as_hole() {
    // Terser with unused key: `const key = pair[0]` becomes `pair[0];`.
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0];
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const [, value] of entries) {
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_folds_discarded_index_from_existing_for_of_ident() {
    let input = r#"
for (const pair of entries) {
  pair[0];
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const [, value] of entries) {
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fold_preserves_nested_write_mutability() {
    let input = r#"
for (const pair of entries) {
  pair[0];
  let value = pair[1];
  out[value = next()] = true;
}
"#;
    let expected = r#"
for (let [, value] of entries) {
  out[value = next()] = true;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fold_preserves_const_write_errors() {
    for write in ["value = next();", "out[value = next()] = true;"] {
        let input = format!(
            r#"
for (const pair of entries) {{
  pair[0];
  const value = pair[1];
  {write}
}}
"#
        );
        assert_eq_normalized(&apply_with_level(&input, RewriteLevel::Standard), &input);
    }
}

#[test]
fn discarded_index_hole_drops_slot_type_annotation() {
    let input = r#"
for (const pair of entries) {
  pair[0];
  const value: string = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const [, value] of entries) {
  use(value);
}
"#;
    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.ts".to_string(),
            ..Default::default()
        },
    )
    .expect("TypeScript input should decompile")
    .code;
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_of_from_sliced_to_array_discards_unused_index_as_hole() {
    let input = r#"
const iterator = _createForOfIteratorHelper(entries);
let step;
try {
  for (iterator.s(); !(step = iterator.n()).done;) {
    const pair = _slicedToArray(step.value, 2);
    pair[0];
    const value = pair[1];
    use(value);
  }
} catch (err) {
  iterator.e(err);
} finally {
  iterator.f();
}
"#;
    let expected = r#"
for (const [, value] of entries) {
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_from_index_form_discards_unused_index_as_hole() {
    let input = r#"for (let i = 0; i < entries.length; i++) { const _entry = entries[i]; _entry[0]; const value = _entry[1]; use(value); }"#;
    let expected = r#"for (const [, value] of entries) { use(value); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_named_key_binding_is_not_a_hole() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  const key = pair[0];
  const value = pair[1];
  use(key, value);
}
"#;
    let expected = r#"
for (const [key, value] of entries) {
  use(key, value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fails_closed_on_non_consecutive_first_slot() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of entries) {
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fails_closed_on_length_between_slots() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  pair.length;
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of entries) {
  pair.length;
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fails_closed_when_temp_used_later() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0];
  const value = pair[1];
  use(pair, value);
}
"#;
    // Slot recovery would keep `pair` live; leave the helper loop untouched.
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_fails_closed_on_later_reread() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0];
  const value = pair[1];
  use(value, pair[0]);
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_fails_closed_on_assignment() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0] = 0;
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of entries) {
  pair[0] = 0;
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fails_closed_on_update() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0]++;
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of entries) {
  pair[0]++;
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fails_closed_on_delete() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  delete pair[0];
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of entries) {
  delete pair[0];
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fails_closed_on_call() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0]();
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of entries) {
  pair[0]();
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fails_closed_on_void() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  void pair[0];
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of entries) {
  void pair[0];
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fails_closed_on_comma_expr() {
    // SimplifySequence splits the comma before UnForOf; the leftover `pair[0];`
    // must not become a hole because `other()` sits between slots.
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0], other();
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of entries) {
  pair[0];
  other();
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fails_closed_on_shadowed_temp_between_slots() {
    // A nested binding with the same spelling is not an outer slot.
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  {
    const pair = other;
    pair[0];
  }
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of entries) {
  {
    const pair = other;
    pair[0];
  }
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fold_succeeds_when_remaining_shadows_temp_name() {
    // After the slots, a nested binding with the same spelling is not a
    // live use of the outer loop ident (sym + ctxt).
    let input = r#"
for (const pair of entries) {
  pair[0];
  const value = pair[1];
  {
    const pair = other;
    use(pair);
  }
  use(value);
}
"#;
    let expected = r#"
for (const [, value] of entries) {
  {
    const pair = other;
    use(pair);
  }
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fold_fails_closed_when_var_left_used_after_loop() {
    // `var` leaks out of the for-of. Dropping `pair` would leave the later
    // read unresolved.
    let input = r#"
for (var pair of entries) {
  pair[0];
  const value = pair[1];
  use(value);
}
use(pair);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_fold_fails_closed_when_var_left_used_in_rhs() {
    // The iterable is not the loop body; `pair` there is a live read of the
    // function-scoped left binding and must keep it.
    let input = r#"
for (var pair of entries(pair)) {
  pair[0];
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_extract_keeps_var_temp_used_after_loop() {
    // `var pair` inside the helper body is function-scoped. Recovering
    // `[, value]` would drop it while the later read still observes it.
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  var pair = step.value;
  pair[0];
  const value = pair[1];
  use(value);
}
use(pair);
"#;
    let expected = r#"
for (var pair of entries) {
  pair[0];
  const value = pair[1];
  use(value);
}
use(pair);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_index_form_keeps_var_temp_used_after_loop() {
    let input = r#"
for (let i = 0; i < entries.length; i++) {
  var _e = entries[i];
  _e[0];
  const value = _e[1];
  use(value);
}
use(_e);
"#;
    let expected = r#"
for (var _e of entries) {
  _e[0];
  const value = _e[1];
  use(value);
}
use(_e);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fold_fails_closed_when_lifted_binding_is_free_in_rhs() {
    // Lifting `value` into the for-of head would put `entries(value)` in TDZ.
    let input = r#"
for (const pair of entries(value)) {
  pair[0];
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_fold_fails_closed_on_eval_in_rhs_mentioning_lifted() {
    let input = r#"
for (const pair of eval("value")) {
  pair[0];
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_extract_fails_closed_when_lifted_binding_is_free_in_iterable() {
    // Helper conversion lifts `value` into the for-of head. The iterable
    // already reads that printed name; ArrayPat would put it in TDZ.
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries(value)); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0];
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of entries(value)) {
  pair[0];
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_extract_fails_closed_on_eval_in_iterable_mentioning_lifted() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(eval("value")); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0];
  const value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of eval("value")) {
  pair[0];
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_index_form_fails_closed_when_lifted_binding_is_free_in_iterable() {
    let input = r#"for (let i = 0, entries_1 = items(value); i < entries_1.length; i++) { const _entry = entries_1[i]; _entry[0]; const value = _entry[1]; use(value); }"#;
    let expected =
        r#"for (const _entry of items(value)) { _entry[0]; const value = _entry[1]; use(value); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_extract_keeps_sliced_to_array_when_var_temp_used_after_loop() {
    // Ident fallback would drop `_slicedToArray` while `pair` still escapes.
    // Keep the helper loop so the converted array identity stays observable.
    let input = r#"
const iterator = _createForOfIteratorHelper(entries);
let step;
try {
  for (iterator.s(); !(step = iterator.n()).done;) {
    var pair = _slicedToArray(step.value, 2);
    pair[0];
    const value = pair[1];
    use(value);
  }
} catch (err) {
  iterator.e(err);
} finally {
  iterator.f();
}
use(pair);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_extract_fails_closed_when_ident_fallback_would_tdz_temp() {
    // ArrayPat is unsound because `value` is free in the iterable. Ident
    // fallback would bind `pair` on the left and put `entries(pair, …)` in TDZ.
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries(pair, value)); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0];
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_index_form_fails_closed_when_ident_fallback_would_tdz_temp() {
    let input = r#"for (let i = 0, entries_1 = items(_entry, value); i < entries_1.length; i++) { const _entry = entries_1[i]; _entry[0]; const value = _entry[1]; use(value); }"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_extract_fails_closed_on_unknown_eval_in_iterable() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(eval(code)); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0];
  const value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_extract_ident_fallback_keeps_original_temp_kind() {
    // Slot `var value` must not leak onto Ident fallback. The original temp is
    // `const pair`; keep that kind when ArrayPat is unsound because of TDZ.
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries(value)); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0];
  var value = pair[1];
  use(value);
}
"#;
    let expected = r#"
for (const pair of entries(value)) {
  pair[0];
  var value = pair[1];
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_index_form_keeps_var_temp_read_from_iterable() {
    // The element temp is declared in the body. A read in the for-init
    // iterable is not an "inside" use; dropping `_e` would leave `items(_e)`
    // resolving to an outer binding.
    let input = r#"for (let i = 0, entries_1 = items(_e); i < entries_1.length; i++) { var _e = entries_1[i]; _e[0]; const value = _e[1]; use(value); }"#;
    let expected = r#"for (var _e of items(_e)) { _e[0]; const value = _e[1]; use(value); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_fold_fails_closed_on_eval_mentioning_temp() {
    // Direct eval can read the loop binding by name. Known source mentioning
    // `pair` must not drop it.
    let input = r#"
for (const pair of entries) {
  pair[0];
  const value = pair[1];
  eval("pair");
  use(value);
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_fold_fails_closed_on_unknown_eval() {
    let input = r#"
for (const pair of entries) {
  pair[0];
  const value = pair[1];
  eval(code);
  use(value);
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_fold_fails_closed_on_with_in_body() {
    let input = r#"
for (const pair of entries) {
  pair[0];
  const value = pair[1];
  with (obj) use(value);
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_fold_keeps_eval_that_does_not_mention_temp() {
    let input = r#"
for (const pair of entries) {
  pair[0];
  const value = pair[1];
  eval("value");
  use(value);
}
"#;
    let expected = r#"
for (const [, value] of entries) {
  eval("value");
  use(value);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn discarded_index_hole_extract_fails_closed_on_eval_mentioning_temp() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0];
  const value = pair[1];
  eval("pair");
  use(value);
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn discarded_index_hole_does_not_emit_elision_only_pattern() {
    let input = r#"
let step;
for (const iterator = _createForOfIteratorHelperLoose(entries); !(step = iterator()).done;) {
  const pair = step.value;
  pair[0];
  pair[1];
  use(other);
}
"#;
    let expected = r#"
for (const pair of entries) {
  pair[0];
  pair[1];
  use(other);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn loose_iterator_helper_does_not_skip_intervening_stmts() {
    // Out of scope (separate leftover): `let step` must stay adjacent to the helper for.
    let input = r#"
let step;
const acc = [];
for (const iterator = _createForOfIteratorHelperLoose(items); !(step = iterator()).done;) {
  const item = step.value;
  acc.push(item);
}
"#;
    let expected = r#"
let step;
const acc = [];
for (const iterator = _createForOfIteratorHelperLoose(items); !(step = iterator()).done;) {
  const item = step.value;
  acc.push(item);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_renames_element_that_shadows_the_iterable() {
    // Terser reuses the parameter name for the loop element. Lifting `e` into
    // the loop head would evaluate the iterable `e` inside the new binding's
    // TDZ (`for (const e of e)` throws), so the element is renamed instead.
    let input = r#"
export function f(e) {
  var normal = true, didError = false, iteratorError;
  try {
    for (var iterator = e[Symbol.iterator](), step; !(normal = (step = iterator.next()).done); normal = true) {
      const e = step.value;
      use(e);
    }
  } catch (err) {
    didError = true;
    iteratorError = err;
  } finally {
    try {
      if (!normal && iterator.return != null) iterator.return();
    } finally {
      if (didError) throw iteratorError;
    }
  }
}
"#;
    let expected = r#"
export function f(e) {
  for (const e_1 of e) {
    use(e_1);
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_from_indexed_loop_renames_element_that_shadows_the_iterable() {
    let input = r#"
export function g(e, e_1) {
  for (let i = 0, arr = e; i < arr.length; i++) {
    const e = arr[i];
    use(e, e_1);
  }
}
"#;
    // `e_1` is already taken inside the loop, so the fresh name skips it.
    let expected = r#"
export function g(e, e_1) {
  for (const e_2 of e) {
    use(e_2, e_1);
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_keeps_var_element_that_shares_the_iterable_name() {
    // A `var` element is hoisted to the function scope: the lowered loop
    // already reassigned the parameter, so `for (var e of e)` is equivalent
    // and no rename is needed.
    let input = r#"
export function g(e) {
  for (var i = 0, arr = e; i < arr.length; i++) {
    var e = arr[i];
    use(e);
  }
}
"#;
    let expected = r#"
export function g(e) {
  for (var e of e) {
    use(e);
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}
