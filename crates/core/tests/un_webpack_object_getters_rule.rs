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

#[test]
fn folds_webpack_require_d_map_inside_a_function_body() {
    // shape: producer webpack@5.111.1 mode=production (Terser). The entry's
    // escaping namespace object stays in the startup IIFE.
    let input = r#"
(()=>{
  "use strict";
  var e = require("./module-100.js");
  var r = {};
  require.r(r);
  require.d(r, {
    alpha: ()=>e.first,
    beta: ()=>e.second
  });
  globalThis.sink = r;
})();
"#;
    let expected = r#"
(()=>{
  "use strict";
  var e = require("./module-100.js");
  var r = {
    get alpha() {
      return e.first;
    },
    get beta() {
      return e.second;
    }
  };
  globalThis.sink = r;
})();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_webpack_require_d_map_inside_a_function_when_target_is_used_first() {
    let input = r#"
function init() {
  var r = {};
  use(r);
  require.r(r);
  require.d(r, {
    alpha: ()=>alpha,
    beta: ()=>beta
  });
  return r;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_webpack_require_d_map_inside_a_function_with_a_local_require() {
    let input = r#"
function init(require) {
  var r = {};
  require.r(r);
  require.d(r, {
    alpha: ()=>alpha,
    beta: ()=>beta
  });
  return r;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn folds_consecutive_define_property_calls_into_getters() {
    let input = r#"
export const ns = {};
Object.defineProperty(ns, "foo", {
  enumerable: true,
  get: ()=>fooValue
});
Object.defineProperty(ns, "bar", {
  enumerable: true,
  get: ()=>barValue
});
"#;
    let expected = r#"
export const ns = {
  get foo() {
    return fooValue;
  },
  get bar() {
    return barValue;
  }
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn folds_define_property_with_function_getter() {
    let input = r#"
export const ns = {};
Object.defineProperty(ns, "alpha", {
  enumerable: true,
  get: function() { return alphaValue; }
});
Object.defineProperty(ns, "beta", {
  enumerable: true,
  get: function() { return betaValue; }
});
"#;
    let expected = r#"
export const ns = {
  get alpha() {
    return alphaValue;
  },
  get beta() {
    return betaValue;
  }
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_single_define_property_call() {
    let input = r#"
export const ns = {};
Object.defineProperty(ns, "only", {
  enumerable: true,
  get: ()=>onlyValue
});
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_define_property_without_enumerable() {
    let input = r#"
export const ns = {};
Object.defineProperty(ns, "foo", {
  get: ()=>fooValue
});
Object.defineProperty(ns, "bar", {
  get: ()=>barValue
});
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_odp_calls_after_intervening_side_effect() {
    let input = r#"
export const ns = {};
sideEffect();
Object.defineProperty(ns, "foo", {
  enumerable: true,
  get: ()=>fooValue
});
Object.defineProperty(ns, "bar", {
  enumerable: true,
  get: ()=>barValue
});
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn stops_odp_collection_on_intervening_statement() {
    let input = r#"
export const ns = {};
Object.defineProperty(ns, "foo", {
  enumerable: true,
  get: ()=>fooValue
});
Object.defineProperty(ns, "bar", {
  enumerable: true,
  get: ()=>barValue
});
sideEffect();
Object.defineProperty(ns, "baz", {
  enumerable: true,
  get: ()=>bazValue
});
"#;
    let expected = r#"
export const ns = {
  get foo() {
    return fooValue;
  },
  get bar() {
    return barValue;
  }
};
sideEffect();
Object.defineProperty(ns, "baz", {
  enumerable: true,
  get: ()=>bazValue
});
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn folds_define_property_in_block_scope() {
    let input = r#"
function init() {
  const ns = {};
  Object.defineProperty(ns, "a", {
    enumerable: true,
    get: ()=>aValue
  });
  Object.defineProperty(ns, "b", {
    enumerable: true,
    get: ()=>bValue
  });
  return ns;
}
"#;
    let expected = r#"
function init() {
  const ns = {
    get a() {
      return aValue;
    },
    get b() {
      return bValue;
    }
  };
  return ns;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_calls_on_a_shadowed_local_object_binding() {
    // A local binding named `Object` is not the global: its defineProperty
    // may do anything, so deleting the calls and folding getters is unsound.
    let input = r#"
function init(Object) {
  const ns = {};
  Object.defineProperty(ns, "a", {
    enumerable: true,
    get: ()=>aValue
  });
  Object.defineProperty(ns, "b", {
    enumerable: true,
    get: ()=>bValue
  });
  return ns;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_define_properties_on_a_shadowed_local_object_binding() {
    let input = r#"
function init(Object) {
  const utils = {};
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
  return utils;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_define_properties_in_module_with_direct_eval() {
    let input = r#"
eval(s);
const utils = {};
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
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_require_d_map_in_module_with_with_statement() {
    let input = r#"
with (o) {}
var r = {};
require.r(r);
require.d(r, {
  a: function() {
    return 1;
  },
  b: function() {
    return 2;
  }
});
"#;
    assert_eq_normalized(&apply(input), input);
}
