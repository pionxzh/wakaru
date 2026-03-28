mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnWebpackDefineGetters;

fn apply(input: &str) -> String {
    render_rule(input, UnWebpackDefineGetters::new)
}

#[test]
fn groups_consecutive_require_d_calls_into_define_properties() {
    let input = r#"
export const utils = {};
require.d(utils, "TASK", ()=>o.e);
require.d(utils, "SAGA_ACTION", ()=>o.c);
require.d(utils, "noop", ()=>o.u);
"#;
    let expected = r#"
export const utils = {};
Object.defineProperties(utils, {
  TASK: {
    enumerable: true,
    get: ()=>o.e
  },
  SAGA_ACTION: {
    enumerable: true,
    get: ()=>o.c
  },
  noop: {
    enumerable: true,
    get: ()=>o.u
  }
});
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_group_single_require_d_call() {
    let input = r#"
const utils = {};
require.d(utils, "TASK", ()=>o.e);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_group_when_target_is_not_empty_object_init() {
    let input = r#"
const utils = createUtils();
require.d(utils, "TASK", ()=>o.e);
require.d(utils, "noop", ()=>o.u);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_group_across_intervening_statement() {
    let input = r#"
const utils = {};
require.d(utils, "TASK", ()=>o.e);
sideEffect();
require.d(utils, "noop", ()=>o.u);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_group_duplicate_property_names() {
    let input = r#"
const utils = {};
require.d(utils, "TASK", ()=>o.e);
require.d(utils, "TASK", ()=>o.c);
"#;
    assert_eq_normalized(&apply(input), input);
}
