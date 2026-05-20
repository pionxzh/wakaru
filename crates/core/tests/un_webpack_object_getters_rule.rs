mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnWebpackObjectGetters;

fn apply(input: &str) -> String {
    render_rule(input, UnWebpackObjectGetters::new)
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

#[test]
fn folds_webpack_require_d_map_into_object_literal_getters() {
    let input = r#"
const integrations = {};
require.r(integrations);
require.d(integrations, {
  FunctionToString() {
    return FunctionToString;
  },
  InboundFilters() {
    return InboundFilters;
  }
});
class FunctionToString {}
class InboundFilters {}
"#;
    let expected = r#"
const integrations = {
  get FunctionToString() {
    return FunctionToString;
  },
  get InboundFilters() {
    return InboundFilters;
  }
};
class FunctionToString {}
class InboundFilters {}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn folds_webpack_require_d_map_after_unrelated_declaration() {
    let input = r#"
const integrations = {};
function helper(value) {
  return value;
}
require.r(integrations);
require.d(integrations, {
  Breadcrumbs() {
    return Breadcrumbs;
  },
  Dedupe() {
    return Dedupe;
  }
});
class Breadcrumbs {}
class Dedupe {}
"#;
    let expected = r#"
const integrations = {
  get Breadcrumbs() {
    return Breadcrumbs;
  },
  get Dedupe() {
    return Dedupe;
  }
};
function helper(value) {
  return value;
}
class Breadcrumbs {}
class Dedupe {}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_webpack_require_d_map_when_target_is_used_before_definition() {
    let input = r#"
const integrations = {};
use(integrations);
require.r(integrations);
require.d(integrations, {
  Breadcrumbs() {
    return Breadcrumbs;
  },
  Dedupe() {
    return Dedupe;
  }
});
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_webpack_require_d_map_without_require_r_marker() {
    let input = r#"
const integrations = {};
require.d(integrations, {
  Breadcrumbs() {
    return Breadcrumbs;
  },
  Dedupe() {
    return Dedupe;
  }
});
"#;
    assert_eq_normalized(&apply(input), input);
}
