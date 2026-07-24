mod common;

use common::{assert_eq_normalized, render, render_rule};
use wakaru_core::rules::{RewriteLevel, UnComputedProperties};

/// Run only the rule, at the given level. The rule is level-gated internally,
/// so `Minimal` exercises the gate.
fn rule(source: &str, level: RewriteLevel) -> String {
    render_rule(source, |_| UnComputedProperties::new(level))
}

fn standard(source: &str) -> String {
    rule(source, RewriteLevel::Standard)
}

#[test]
fn folds_babel_loose_computed_properties() {
    // Babel 7 loose output for `var n = { [k]: 1, [j]: 2 };`
    let input = r#"
var _n;
var n = (_n = {}, _n[k] = 1, _n[j] = 2, _n);
"#;
    let expected = r#"
var n = {
    [k]: 1,
    [j]: 2
};
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn folds_issue_69_lookup_set() {
    // github.com/pionxzh/wakaru/issues/69 — the reported shape, with the
    // `var _n;` declaration Babel emits alongside it.
    let input = r#"
var _n;
var n = (_n = {}, _n[34] = true, _n[20] = true, _n[31] = true, _n[22] = true, _n[71] = true, _n);
console.log(n);
"#;
    let expected = r#"
var n = {
    34: true,
    20: true,
    31: true,
    22: true,
    71: true
};
console.log(n);
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn keeps_seed_properties_before_the_first_computed_key() {
    // Babel keeps properties preceding the first computed key in the seed
    // literal and lowers everything from that key onward.
    let input = r#"
var _n;
var n = (_n = { a: 1 }, _n[k] = 2, _n.b = 3, _n["c-d"] = 4, _n[5] = 6, _n);
"#;
    let expected = r#"
var n = {
    a: 1,
    [k]: 2,
    b: 3,
    "c-d": 4,
    5: 6
};
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn folds_nested_lowerings_from_the_inside_out() {
    // `var n = { [k]: { [j]: 1 } };`
    let input = r#"
var _k, _n;
var n = (_n = {}, _n[k] = (_k = {}, _k[j] = 1, _k), _n);
"#;
    let expected = r#"
var n = {
    [k]: {
        [j]: 1
    }
};
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn folds_in_call_argument_and_return_positions() {
    let input = r#"
var _f;
f((_f = {}, _f[k] = 1, _f.b = 2, _f));
function g() {
    var _ref;
    return _ref = {}, _ref[k] = 1, _ref;
}
"#;
    let expected = r#"
f({
    [k]: 1,
    b: 2
});
function g() {
    return {
        [k]: 1
    };
}
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn folds_string_keys_into_string_property_names() {
    // The rule emits `PropName::Str`; `UnBracketNotation` runs later in the
    // pipeline and normalizes identifier-shaped ones (see the pipeline test).
    let input = r#"
var _n;
var n = (_n = {}, _n["alpha"] = 1, _n["b c"] = 2, _n);
"#;
    let expected = r#"
var n = {
    "alpha": 1,
    "b c": 2
};
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn preserves_key_and_value_evaluation_order() {
    // Both forms evaluate each key before its own value, and the pairs in
    // source order, so side-effecting keys are safe to fold.
    let input = r#"
var _n;
var n = (_n = {}, _n[f()] = g(), _n[h()] = i(), _n);
"#;
    let expected = r#"
var n = {
    [f()]: g(),
    [h()]: i()
};
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn folds_only_the_object_building_suffix_of_a_merged_sequence() {
    // A minifier can comma-merge unrelated work ahead of the pattern.
    let input = r#"
var _n;
var n = (setup(), _n = {}, _n[k] = 1, _n);
"#;
    let expected = r#"
var n = (setup(), {
    [k]: 1
});
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn keeps_parenthesized_elements() {
    // The shape as pasted in issue 69, with each sequence element parenthesized.
    let input = r#"
var _n;
var n = ((_n = {}), (_n[34] = true), (_n[20] = true), _n);
"#;
    let expected = r#"
var n = {
    34: true,
    20: true
};
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

// ---------------------------------------------------------------------------
// Chained seed — what Terser's `compress` pass produces
// ---------------------------------------------------------------------------

#[test]
fn folds_a_chained_seed() {
    // `compress` folds `_n = {}, _n[k] = 1` into `(_n = {})[k] = 1`.
    let input = r#"
var _n;
var n = ((_n = {})[k] = 1, _n.b = 2, _n);
"#;
    let expected = r#"
var n = {
    [k]: 1,
    b: 2
};
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn folds_a_chained_seed_carrying_leading_properties() {
    let input = r#"
var _n;
var n = ((_n = { a: 0 })[k] = 1, _n.b = 2, _n);
"#;
    let expected = r#"
var n = {
    a: 0,
    [k]: 1,
    b: 2
};
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn folds_a_two_element_chained_sequence() {
    // A single computed property compresses to just seed + read.
    let input = r#"
var _n;
var n = ((_n = {})[k] = 1, _n);
"#;
    let expected = r#"
var n = {
    [k]: 1
};
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn skips_a_chained_seed_onto_another_object() {
    let input = r#"
var _n;
var n = ((other = {})[k] = 1, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_chained_seed_with_a_proto_key() {
    let input = r#"
var _n;
var n = ((_n = {}).__proto__ = p, _n.b = 2, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn pipeline_recovers_a_compressed_and_mangled_lookup_set() {
    // Babel loose output run through Terser with compress + mangle, the shape a
    // production bundle actually carries.
    let input = r#"
export function f() {
    var r;
    return (r = {})[34] = !0, r[20] = !0, r[31] = !0, r;
}
"#;
    let expected = r#"
export function f() {
    return {
        34: true,
        20: true,
        31: true
    };
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ---------------------------------------------------------------------------
// Bail-outs
// ---------------------------------------------------------------------------

#[test]
fn skips_an_undeclared_temp() {
    // Without a declaration the temp is a global (sloppy mode: erasing the
    // assignment drops a global write) or unresolved (drops a ReferenceError).
    // This is the snippet exactly as pasted in issue 69.
    let input = r#"
var n = ((a = {}), (a[34] = true), (a[20] = true), a);
"#;
    // Unchanged apart from the fixer dropping the redundant parentheses.
    let expected = r#"
var n = (a = {}, a[34] = true, a[20] = true, a);
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn skips_a_temp_observed_outside_the_pattern() {
    let input = r#"
var _n;
var n = (_n = {}, _n[k] = 1, _n);
console.log(_n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_temp_referenced_by_a_value() {
    // `{ [k]: _n }` would read an unassigned temp instead of the seed object.
    let input = r#"
var _n;
var n = (_n = {}, _n[k] = _n, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_temp_read_before_the_seed() {
    let input = r#"
var _n;
var n = (use(_n), _n = {}, _n[k] = 1, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_an_initialized_temp() {
    // An initialized declaration is not a Babel temp, and its initial value
    // could be observed between the declaration and the sequence.
    let input = r#"
var _n = seed();
var n = (_n = {}, _n[k] = 1, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_later_lexical_temp_to_preserve_tdz() {
    // Babel emits `var`, not `let`. A later lexical declaration is still in its
    // TDZ here, so the original assignment throws before constructing an object.
    let input = r#"
let n = (_n = {}, _n.x = 1, _n);
let _n;
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn folds_a_later_var_temp_because_it_is_hoisted() {
    let input = r#"
var n = (_n = {}, _n.x = 1, _n);
var _n;
"#;
    let expected = r#"
var n = {
    x: 1
};
"#;
    assert_eq_normalized(&standard(input), expected.trim());
}

#[test]
fn skips_a_directly_exported_temp() {
    // A direct export is observable without another identifier occurrence:
    // importers see `_n` become the same object as `n` after module evaluation.
    let input = r#"
export var _n;
export var n = (_n = {}, _n.x = 1, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_proto_assignment() {
    // `_n.__proto__ = p` invokes the inherited setter; `{ __proto__: p }` in a
    // computed key position defines an own property.
    let input = r#"
var _n;
var n = (_n = {}, _n[k] = 1, _n.__proto__ = p, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_string_proto_assignment() {
    let input = r#"
var _n;
var n = (_n = {}, _n["__proto__"] = 1, _n.x = 2, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_no_substitution_template_proto_assignment() {
    // This key is just as statically known as the string-literal spelling.
    let input = r#"
var _n;
var n = (_n = {}, _n[`__proto__`] = p, _n.x = 2, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_chained_no_substitution_template_proto_assignment() {
    let input = r#"
var _n;
var n = ((_n = {})[`__proto__`] = p, _n.x = 2, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_seed_with_an_accessor() {
    // Assigning to a key the seed covers with an accessor calls that accessor.
    let input = r#"
var _n;
var n = (_n = {
    get a () {
        return 1;
    }
}, _n.a = 2, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_seed_with_a_proto_key() {
    // The seed installs a prototype whose setters the later assignment hits.
    let input = r#"
var _n;
var n = (_n = {
    __proto__: p
}, _n[k] = 1, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_compound_assignment() {
    let input = r#"
var _n;
var n = (_n = {}, _n[k] += 1, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_an_assignment_onto_another_object() {
    let input = r#"
var _n;
var n = (_n = {}, other[k] = 1, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_seed_without_any_assignment() {
    let input = r#"
var _n;
var n = (_n = {}, side(), _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn skips_a_non_object_seed() {
    let input = r#"
var _n;
var n = (_n = [], _n[0] = 1, _n);
"#;
    assert_eq_normalized(&standard(input), input.trim());
}

#[test]
fn minimal_level_preserves_the_sequence() {
    let input = r#"
var _n;
var n = (_n = {}, _n[k] = 1, _n);
"#;
    assert_eq_normalized(&rule(input, RewriteLevel::Minimal), input.trim());
}

#[test]
fn standard_explicitly_assumes_anonymous_value_name_inference_is_unobserved() {
    // Assignment leaves both `.name` values empty, while object-literal named
    // evaluation infers `fn` and `Class`. This is deliberately covered by the
    // standard-only `set_computed_properties` assumption.
    let input = r#"
var _n;
var n = (_n = {}, _n.fn = function() {}, _n.Class = class {}, _n);
"#;
    let expected = r#"
var n = {
    fn: function() {},
    Class: class {}
};
"#;
    assert_eq_normalized(&standard(input), expected.trim());
    assert_eq_normalized(&rule(input, RewriteLevel::Minimal), input.trim());
}

// ---------------------------------------------------------------------------
// Full pipeline
// ---------------------------------------------------------------------------

#[test]
fn pipeline_recovers_the_issue_69_object_literal() {
    let input = r#"
var _n;
var n = (_n = {}, _n[34] = true, _n[20] = true, _n[31] = true, _n[22] = true, _n[71] = true, _n);
console.log(n);
"#;
    let expected = r#"
const n = {
    34: true,
    20: true,
    31: true,
    22: true,
    71: true
};
console.log(n);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn pipeline_normalizes_string_keys() {
    let input = r#"
var _n;
var n = (_n = {}, _n["alpha"] = 1, _n["b c"] = 2, _n[k] = 3, _n);
console.log(n);
"#;
    let expected = r#"
const n = {
    alpha: 1,
    "b c": 2,
    [k]: 3
};
console.log(n);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn pipeline_recovers_a_mangled_lowering() {
    // Babel loose output run through Terser with mangle only.
    let input = r#"
function build() {
    var r;
    var u = (r = {}, r[34] = true, r[20] = true, r[71] = true, r);
    return u;
}
"#;
    // The surviving `u` alias is unrelated to this rule — SmartInline keeps
    // single-read aliases whose name may still carry recovered intent.
    let expected = r#"
function build() {
    const u = {
        34: true,
        20: true,
        71: true
    };
    return u;
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}
