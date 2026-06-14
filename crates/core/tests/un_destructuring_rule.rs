mod common;

use common::{assert_eq_normalized, render_pipeline_until_with_level, render_rule};
use wakaru_core::rules::UnDestructuring;
use wakaru_core::RewriteLevel;

fn apply(input: &str) -> String {
    render_rule(input, UnDestructuring::new)
}

#[test]
fn reconstructs_array_rest_from_ref_slice() {
    let input = r#"
var _ref = arr;
var head = _ref[0];
var tail = _ref.slice(1);
"#;
    let expected = r#"
var [head, ...tail] = arr;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_array_rest_with_holes() {
    let input = r#"
var _ref = arr;
var first = _ref[0];
var third = _ref[2];
var rest = _ref.slice(3);
"#;
    let expected = r#"
var [first, , third, ...rest] = arr;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn rejects_array_rest_when_slice_has_end_arg() {
    let input = r#"
var _ref = arr;
var head = _ref[0];
var tail = _ref.slice(1, 3);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn rejects_array_rest_when_later_index_is_inside_rest() {
    let input = r#"
var _ref = arr;
var head = _ref[0];
var tail = _ref.slice(1);
var third = _ref[2];
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn reconstructs_array_default_from_temp_conditional() {
    let input = r#"
var _ref = arr;
var _tmp = _ref[0];
var head = _tmp === void 0 ? "default" : _tmp;
var tail = _ref.slice(1);
"#;
    let expected = r#"
var [head = "default", ...tail] = arr;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_object_default_from_temp_conditional() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.foo;
var foo = _tmp === void 0 ? 1 : _tmp;
var bar = _ref.bar;
"#;
    let expected = r#"
var { foo = 1, bar } = opts;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_object_alias_default_from_temp_conditional() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.foo;
var value = _tmp === void 0 ? 1 : _tmp;
var label = _ref.label;
"#;
    let expected = r#"
var { foo: value = 1, label } = opts;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn rejects_default_that_uses_removed_ref_binding() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.foo;
var foo = _tmp === void 0 ? _ref.bar : _tmp;
var bar = _ref.bar;
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn rejects_default_that_uses_previous_removed_temp() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.foo;
var foo = _tmp === void 0 ? 1 : _tmp;
var _tmp2 = _ref.bar;
var bar = _tmp2 === void 0 ? _tmp : _tmp2;
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn reconstructs_object_default_false_from_temp_logical_and() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.exact;
var exact = _tmp !== undefined && _tmp;
var strict = _ref.strict;
"#;
    let expected = r#"
var { exact = false, strict } = opts;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_object_default_true_from_temp_logical_or() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.pure;
var pure = _tmp === undefined || _tmp;
var mode = _ref.mode;
"#;
    let expected = r#"
var { pure = true, mode } = opts;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_object_alias_default_false_from_reversed_undefined_check() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.exact;
var enabled = undefined !== _tmp && _tmp;
var strict = _ref.strict;
"#;
    let expected = r#"
var { exact: enabled = false, strict } = opts;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn rejects_group_when_ref_is_used_later() {
    let input = r#"
var _ref = arr;
var head = _ref[0];
consume(_ref);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn leaves_plain_index_groups_to_smart_inline() {
    let input = r#"
var _ref = arr;
var first = _ref[0];
var second = _ref[1];
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn reconstructs_rest_after_spread_array_unwrap() {
    let input = r#"
var _ref = [...arr];
var head = _ref[0];
var tail = _ref.slice(1);
"#;
    let expected = r#"
var [head, ...tail] = arr;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_array_rest_from_array_like_to_array_slice() {
    let input = r#"
var _ref = arr;
var head = _ref[0];
var tail = _arrayLikeToArray(_ref).slice(1);
"#;
    let expected = r#"
var [head, ...tail] = arr;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_array_default_rest_from_array_like_to_array_slice() {
    let input = r#"
var _ref = arr;
var head = _ref[0];
var _tmp = _ref[2];
var third = _tmp === void 0 ? fallback : _tmp;
var tail = _arrayLikeToArray(_ref).slice(3);
"#;
    let expected = r#"
var [head, , third = fallback, ...tail] = arr;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn leaves_direct_loose_array_rest() {
    let input = r#"
const head = values[0];
const rest = values.slice(1);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn leaves_direct_loose_array_rest_after_multiple_indexes() {
    let input = r#"
const head = ref[0];
const neck = ref[1];
const tail = ref.slice(2);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn rejects_direct_loose_array_rest_when_explicit_index_overlaps_rest() {
    let input = r#"
const head = ref[0];
const neck = ref[1];
const tail = ref.slice(1);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn leaves_direct_loose_array_rest_with_default_temp() {
    let input = r#"
const first = items[0];
const _a = items[2];
const second = _a === void 0 ? fallback : _a;
const rest_items = items.slice(3);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn leaves_direct_slice_without_index_access() {
    let input = r#"
const rest = values.slice(1);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn reconstructs_tsc_array_rest_from_split_var_decl() {
    let input = r#"
var first = items[0], rest_items = items.slice(1);
use(first, rest_items);
"#;
    let expected = r#"
const [first, ...rest_items] = items;
use(first, rest_items);
"#;
    assert_eq_normalized(
        &render_pipeline_until_with_level(input, "UnDestructuring", RewriteLevel::Aggressive),
        expected,
    );
}

#[test]
fn standard_preserves_direct_array_rest_from_split_var_decl() {
    let input = r#"
var first = items[0], rest_items = items.slice(1);
use(first, rest_items);
"#;
    let expected = r#"
const first = items[0];
const rest_items = items.slice(1);
use(first, rest_items);
"#;
    assert_eq_normalized(
        &render_pipeline_until_with_level(input, "UnDestructuring", RewriteLevel::Standard),
        expected,
    );
}

#[test]
fn reconstructs_tsc_array_rest_with_default_and_hole() {
    let input = r#"
var first = items[0], _a = items[2], second = _a === void 0 ? fallback : _a, rest_items = items.slice(3);
use(first, second, rest_items);
"#;
    let expected = r#"
const [first, , second = fallback, ...rest_items] = items;
use(first, second, rest_items);
"#;
    assert_eq_normalized(
        &render_pipeline_until_with_level(input, "UnDestructuring", RewriteLevel::Aggressive),
        expected,
    );
}

#[test]
fn standard_preserves_direct_array_rest_with_default_and_hole() {
    let input = r#"
var first = items[0], _a = items[2], second = _a === void 0 ? fallback : _a, rest_items = items.slice(3);
use(first, second, rest_items);
"#;
    let expected = r#"
const first = items[0];
const _a = items[2];
const second = _a === undefined ? fallback : _a;
const rest_items = items.slice(3);
use(first, second, rest_items);
"#;
    assert_eq_normalized(
        &render_pipeline_until_with_level(input, "UnDestructuring", RewriteLevel::Standard),
        expected,
    );
}

#[test]
fn standard_preserves_potential_object_slice_semantics() {
    let input = r#"
var first = source[0], rest = source.slice(1);
use(first, rest);
"#;
    let expected = r#"
const first = source[0];
const rest = source.slice(1);
use(first, rest);
"#;
    assert_eq_normalized(
        &render_pipeline_until_with_level(input, "UnDestructuring", RewriteLevel::Standard),
        expected,
    );
}

#[test]
fn nests_destructuring_default_in_ref_group() {
    let input = r#"
var _ref = source;
var _a = _ref.outer;
var _b = _a === void 0 ? {} : _a;
var _c = _b.value;
var result = _c === void 0 ? fallback : _c;
use(result);
"#;
    let expected = r#"
var { outer: { value: result = fallback } = {} } = source;
use(result);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_assignment_object_with_nested_defaults() {
    let input = r#"
let source;
let id;
let _b;
let _c;
let name;
let _d;
let _e;
let primary;
let backup;
source = input;
id = source.id;
_b = source.profile;
_c = _b === undefined ? {} : _b;
name = _c.name;
_d = source.tags;
_e = _d === undefined ? [] : _d;
primary = _e[0];
backup = _e[2];
use(id, name, primary, backup);
"#;
    let expected = r#"
let source;
let id;
let _b;
let _c;
let name;
let _d;
let _e;
let primary;
let backup;
source = input;
({ id, profile: { name } = {}, tags: [primary, , backup] = [] } = source);
use(id, name, primary, backup);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_assignment_object_with_fused_nested_defaults() {
    // After terser-compress the defaulted temp is fused into the first access:
    // `name = (_c = _b === undefined ? {} : _b).name` and
    // `primary = (_e = _d === undefined ? [] : _d)[0]; backup = _e[2]`.
    let input = r#"
let source;
let id;
let _b;
let _c;
let name;
let _d;
let _e;
let primary;
let backup;
source = input;
id = source.id;
_b = source.profile;
name = (_c = _b === undefined ? {} : _b).name;
_d = source.tags;
primary = (_e = _d === undefined ? [] : _d)[0];
backup = _e[2];
use(id, name, primary, backup);
"#;
    let expected = r#"
let source;
let id;
let _b;
let _c;
let name;
let _d;
let _e;
let primary;
let backup;
source = input;
({ id, profile: { name } = {}, tags: [primary, , backup] = [] } = source);
use(id, name, primary, backup);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_assignment_object_from_fused_member_default_access() {
    let input = r#"
let source;
let tmp;
let _c;
let name;
source = input;
tmp = source.profile;
name = (_c = tmp === undefined ? {} : tmp).name;
use(name);
"#;
    let expected = r#"
let source;
let tmp;
let _c;
let name;
source = input;
({ profile: { name } = {} } = source);
use(name);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_array_when_tail_binding_fused_into_conditional() {
    // Minifiers inline an extracted array element into its first use:
    // `_f = (backup = _e[2]) != null ? backup : fallback()`. The embedded
    // `backup = _e[2]` assignment is hoisted so the array pattern completes.
    let input = r#"
let source;
let _d;
let _e;
let primary;
let backup;
let _f;
source = input;
_d = source.tags;
primary = (_e = _d === undefined ? [] : _d)[0];
_f = (backup = _e[2]) != null ? backup : fallback();
use(primary, backup, _f);
"#;
    let expected = r#"
let source;
let _d;
let _e;
let primary;
let backup;
let _f;
source = input;
({ tags: [primary, , backup] = [] } = source);
_f = backup != null ? backup : fallback();
use(primary, backup, _f);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_object_default_with_inline_established_source() {
    // The destructuring source is established inline inside the fused access:
    // `J = (Z = (V = G ?? {}).link) === undefined ? def : Z` — the member
    // object `(V = G ?? {})` is an assignment, so the hoist fires and the group
    // destructures from `V`.
    let input = r#"
let J;
let Z;
let V;
J = (Z = (V = G ?? {}).link) === undefined ? def : Z;
use(J, V);
"#;
    let expected = r#"
let J;
let Z;
let V;
({ link: J = def } = V = G ?? {});
use(J, V);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_hoist_conditional_test_assignment_without_destructuring_source() {
    // The hoist must only fire when the member object is a binding (`Z`) or an
    // inline-established one (`(V = ...)`). For an arbitrary expression there is
    // no destructuring group to form, so splitting the statement would be churn.
    // The self-assign idiom (`o = (o = t.state) ...`) is likewise left intact.
    let input = r#"
let x;
let c;
let o;
let f;
let g;
f = (x = (a ?? {}).link) != null ? x : y;
g = (c = e.opts.flag) === true ? p : q;
o = (o = t.field) !== null ? o.value : null;
use(f, g, o);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn reconstructs_assignment_object_from_member_default_access() {
    let input = r#"
let source;
let tmp;
let name;
source = input;
tmp = source.profile;
name = (tmp === undefined ? {} : tmp).name;
use(name);
"#;
    let expected = r#"
let source;
let tmp;
let name;
source = input;
({ profile: { name } = {} } = source);
use(name);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_assignment_array_from_sliced_default_access() {
    let input = r#"
let source;
let tmp;
let _ref;
let primary;
let backup;
source = input;
tmp = source.tags;
_ref = _sliced_to_array(tmp === undefined ? [] : tmp, 3);
primary = _ref[0];
backup = _ref[2];
use(primary, backup);
"#;
    let expected = r#"
let source;
let tmp;
let _ref;
let primary;
let backup;
source = input;
({ tags: [primary, , backup] = [] } = source);
use(primary, backup);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn rejects_assignment_object_when_removed_temp_is_used_later() {
    let input = r#"
let source;
let id;
let _b;
let _c;
let name;
source = input;
id = source.id;
_b = source.profile;
_c = _b === undefined ? {} : _b;
name = _c.name;
use(id, name, _b);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn nests_param_destructuring_default_babel() {
    let input = r#"
function nested() {
    var _ref = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : {};
    var _ref$outer = _ref.outer;
    var _ref$outer2 = _ref$outer === void 0 ? {} : _ref$outer;
    var _ref$outer2$value = _ref$outer2.value;
    var value = _ref$outer2$value === void 0 ? fallbackValue : _ref$outer2$value;
    return use(value);
}
"#;
    let expected = r#"
function nested({ outer: { value = fallbackValue } = {} } = {}) {
    return use(value);
}
"#;
    assert_eq_normalized(
        &render_pipeline_until_with_level(input, "UnDestructuring", RewriteLevel::Standard),
        expected,
    );
}

#[test]
fn nests_param_destructuring_default_tsc() {
    let input = r#"
function nested(_a) {
    var _b = _a === void 0 ? {} : _a, _c = _b.outer, _d = _c === void 0 ? {} : _c, _e = _d.value, value = _e === void 0 ? fallbackValue : _e;
    return use(value);
}
"#;
    let expected = r#"
function nested({ outer: { value = fallbackValue } = {} } = {}) {
    return use(value);
}
"#;
    assert_eq_normalized(
        &render_pipeline_until_with_level(input, "UnParameters2", RewriteLevel::Standard),
        expected,
    );
}

#[test]
fn sliced_to_array_folded_into_object_destructuring_assignment() {
    let input = r#"
source = _t;
id = source.id;
_source$profile = source.profile;
_source$profile2 = _source$profile === void 0 ? {} : _source$profile;
name = _source$profile2.name;
_source$tags = source.tags;
_source$tags2 = _source$tags === void 0 ? [] : _source$tags;
_source$tags3 = _slicedToArray(_source$tags2, 3);
primary = _source$tags3[0];
backup = _source$tags3[2];
"#;
    let expected = r#"
source = _t;
({
  id,
  profile: { name } = {},
  tags: [primary, , backup] = []
} = source);
"#;
    assert_eq_normalized(&apply(input), expected);
}
