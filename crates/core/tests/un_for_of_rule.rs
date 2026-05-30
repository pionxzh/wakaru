mod common;

use common::{assert_eq_normalized, render, render_rule};
use wakaru_core::{rules::UnForOf, RewriteLevel};

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |_| UnForOf::new(level))
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
for (const item of items) {
  use(item);
}
"#;
    assert_eq_normalized(&render(input), expected);
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
