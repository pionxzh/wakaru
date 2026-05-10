mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnWebpackObjectGetters;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnWebpackObjectGetters)
}

#[test]
fn folds_define_properties_into_object_literal_getters() {
    let input = r#"
export const utils = {};
Object.defineProperties(utils, {
  TASK: {
    enumerable: true,
    get: ()=>o.e
  },
  SAGA_ACTION: {
    enumerable: true,
    get: ()=>o.c
  }
});
"#;
    let expected = r#"
export const utils = {
  get TASK() {
    return o.e;
  },
  get SAGA_ACTION() {
    return o.c;
  }
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_non_exact_descriptors() {
    let input = r#"
const utils = {};
Object.defineProperties(utils, {
  TASK: {
    enumerable: true,
    configurable: true,
    get: ()=>o.e
  },
  noop: {
    enumerable: true,
    get: ()=>o.u
  }
});
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_non_getter_define_properties_blocks() {
    let input = r#"
const utils = {};
Object.defineProperties(utils, {
  TASK: {
    enumerable: true,
    value: o.e
  },
  noop: {
    enumerable: true,
    get: ()=>o.u
  }
});
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_named_or_parameterized_getters() {
    let input = r#"
const utils = {};
Object.defineProperties(utils, {
  TASK: {
    enumerable: true,
    get: function taskGetter() {
      return o.e;
    }
  },
  noop: {
    enumerable: true,
    get: (value)=>value
  }
});
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_when_not_immediately_following_empty_object_init() {
    let input = r#"
const utils = {};
sideEffect();
Object.defineProperties(utils, {
  TASK: {
    enumerable: true,
    get: ()=>o.e
  },
  noop: {
    enumerable: true,
    get: ()=>o.u
  }
});
"#;
    assert_eq_normalized(&apply(input), input);
}
