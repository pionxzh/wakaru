mod common;

use common::{assert_eq_normalized, render, render_rule};
use wakaru_core::driver::test_support::unpack;
use wakaru_core::rules::UnEsbuildKeepNames;
use wakaru_core::{decompile, DecompileOptions};

fn apply(input: &str) -> String {
    render_rule(input, UnEsbuildKeepNames::new)
}

const HELPER: &str = r#"
var __defProp = Object.defineProperty;
var __name = (target, value) => __defProp(target, "name", { value, configurable: true });
"#;

fn with_helper(body: &str) -> String {
    format!("{HELPER}{body}")
}

#[test]
fn drops_wrappers_on_declarators_whose_binding_names_the_value() {
    let input = with_helper(
        r#"
const twice = __name(function (n) { return n * 2; }, "twice");
let thrice = __name((n) => n * 3, "thrice");
var Box = class { static { __name(this, "Box"); } open() {} };
use(twice, thrice, Box);
"#,
    );
    let expected = r#"
const twice = function (n) { return n * 2; };
let thrice = (n) => n * 3;
var Box = class { open() {} };
use(twice, thrice, Box);
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn drops_statement_wrappers_after_function_and_class_declarations() {
    let input = with_helper(
        r#"
function quad(n) { return n * 4; }
__name(quad, "quad");
class Lamp { static { __name(this, "Lamp"); } on() {} }
export function half(n) { return n / 2; }
__name(half, "half");
use(quad(1), new Lamp());
export { quad, Lamp };
"#,
    );
    let expected = r#"
function quad(n) { return n * 4; }
class Lamp { on() {} }
export function half(n) { return n / 2; }
use(quad(1), new Lamp());
export { quad, Lamp };
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn drops_wrappers_in_other_naming_positions() {
    let input = with_helper(
        r##"
let late;
late = __name(function () {}, "late");
let lazy;
lazy ||= __name(() => 5, "lazy");
const bag = { pick: __name(function () {}, "pick"), "two words": __name(() => 1, "two words") };
class Tray {
  slot = __name(() => 1, "slot");
  static make = __name(function () {}, "make");
  #hid = __name(() => 2, "#hid");
}
function read(src) {
  const { take = __name(() => 4, "take") } = src;
  const [first = __name(() => 3, "first")] = src;
  return [take, first];
}
use(late, lazy, bag, Tray, read);
"##,
    );
    let expected = r##"
let late;
late = function () {};
let lazy;
lazy ||= () => 5;
const bag = { pick: function () {}, "two words": () => 1 };
class Tray {
  slot = () => 1;
  static make = function () {};
  #hid = () => 2;
}
function read(src) {
  const { take = () => 4 } = src;
  const [first = () => 3] = src;
  return [take, first];
}
use(late, lazy, bag, Tray, read);
"##;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn drops_wrapper_on_a_named_function_expression_anywhere() {
    let input = with_helper(
        r#"
use(__name(function spin() {}, "spin"));
const Cls = __name(class Cls {}, "Cls");
"#,
    );
    let expected = r#"
use(function spin() {});
const Cls = class Cls {};
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn keeps_the_exported_default_call() {
    let input = with_helper(
        r#"
export default __name(function fmt() {}, "fmt");
"#,
    );
    assert_eq_normalized(&apply(&input), &input);
}

#[test]
fn recognizes_the_minified_helper_in_an_iife() {
    let input = r#"
(() => {
  var t = Object.defineProperty;
  var n = (e, r) => t(e, "name", { value: r, configurable: !0 });
  var dbl = n(function (e) { return e * 2; }, "dbl");
  function quad(e) { return e * 4; }
  n(quad, "quad");
  var Box = class { static { n(this, "Box"); } };
  use(dbl, quad, Box);
})();
"#;
    let expected = r#"
(() => {
  var dbl = function (e) { return e * 2; };
  function quad(e) { return e * 4; }
  var Box = class {};
  use(dbl, quad, Box);
})();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn recognizes_the_helper_calling_object_define_property_directly() {
    let input = r#"
const label = (target, value) => Object.defineProperty(target, "name", { value, configurable: true });
const go = label(() => 1, "go");
use(go);
"#;
    let expected = r#"
const go = () => 1;
use(go);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_wrappers_whose_name_the_language_would_not_give() {
    let input = with_helper(
        r#"
const a = __name(function () {}, "b");
const c = __name(function other() {}, "c");
const obj = {};
obj.x = __name(function () {}, "x");
const keyed = { [k]: __name(() => 1, "k"), __proto__: __name(() => 2, "__proto__"), 7: __name(() => 3, "7") };
function plain() {}
__name(plain, "renamed");
use(__name(() => 1, "loose"), __name(class {}, "Anon"));
"#,
    );
    assert_eq_normalized(&apply(&input), &input);
}

#[test]
fn keeps_static_name_blocks_a_class_member_could_override() {
    let input = with_helper(
        r#"
class Own { static name() { return "own"; } static { __name(this, "Own"); } }
class Dyn { static [key] = 1; static { __name(this, "Dyn"); } }
class Late { static count = 1; static { __name(this, "Late"); } }
class Other { static { __name(this, "Else"); } }
use(Own, Dyn, Late, Other);
"#,
    );
    assert_eq_normalized(&apply(&input), &input);
}

#[test]
fn keeps_statement_wrappers_when_earlier_code_can_reach_the_function() {
    let input = with_helper(
        r#"
function early() {}
register(early);
__name(early, "early");
function hoisted() {}
__name(hoisted, "hoisted");
function expose() { return hoisted; }
function moved() {}
moved = other;
__name(moved, "moved");
__name(before, "before");
function before() {}
class Static { static { init(this); } }
__name(Static, "Static");
"#,
    );
    assert_eq_normalized(&apply(&input), &input);
}

#[test]
fn drops_statement_wrapper_when_earlier_code_only_calls_the_function() {
    let input = with_helper(
        r#"
function step() { return step.count++; }
step();
new step();
typeof step;
__name(step, "step");
use(step);
"#,
    );
    let expected = r#"
function step() { return step.count++; }
step();
new step();
typeof step;
use(step);
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn keeps_look_alike_and_written_helpers() {
    let writable = r#"
var tag = (target, value) => Object.defineProperty(target, "name", { value, writable: true });
const a = tag(() => 1, "a");
"#;
    assert_eq_normalized(&apply(writable), writable);

    let display_name = r#"
var tag = (target, value) => Object.defineProperty(target, "displayName", { value, configurable: true });
const a = tag(() => 1, "a");
"#;
    assert_eq_normalized(&apply(display_name), display_name);

    let reassigned = r#"
var tag = (target, value) => Object.defineProperty(target, "name", { value, configurable: true });
tag = (target) => target;
const a = tag(() => 1, "a");
"#;
    assert_eq_normalized(&apply(reassigned), reassigned);

    let local_object = r#"
const Object = { defineProperty: (target) => target };
const tag = (target, value) => Object.defineProperty(target, "name", { value, configurable: true });
const a = tag(() => 1, "a");
"#;
    assert_eq_normalized(&apply(local_object), local_object);

    let written_alias = r#"
var def = Object.defineProperty;
def = other;
var tag = (target, value) => def(target, "name", { value, configurable: true });
const a = tag(() => 1, "a");
"#;
    assert_eq_normalized(&apply(written_alias), written_alias);
}

#[test]
fn keeps_the_helper_while_a_wrapper_still_calls_it() {
    let input = with_helper(
        r#"
const a = __name(() => 1, "a");
const b = __name(() => 2, "c");
"#,
    );
    let expected = with_helper(
        r#"
const a = () => 1;
const b = __name(() => 2, "c");
"#,
    );
    assert_eq_normalized(&apply(&input), &expected);
}

#[test]
fn keeps_the_alias_when_another_helper_uses_it() {
    let input = r#"
var __defProp = Object.defineProperty;
var __name = (target, value) => __defProp(target, "name", { value, configurable: true });
var __export = (target, all) => { for (var name in all) __defProp(target, name, { get: all[name], enumerable: true }); };
const a = __name(() => 1, "a");
use(__export);
"#;
    let expected = r#"
var __defProp = Object.defineProperty;
var __export = (target, all) => { for (var name in all) __defProp(target, name, { get: all[name], enumerable: true }); };
const a = () => 1;
use(__export);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn leaves_modules_with_direct_eval_alone() {
    let input = with_helper(
        r#"
const a = __name(() => 1, "a");
eval(code);
"#,
    );
    assert_eq_normalized(&apply(&input), &input);
}

// producer esbuild@0.28.0 --bundle --keep-names --format=esm
#[test]
fn pipeline_recovers_esbuild_esm_keep_names_output() {
    let input = r#"
var __defProp = Object.defineProperty;
var __name = (target, value) => __defProp(target, "name", { value, configurable: true });
var dbl = /* @__PURE__ */ __name(function(x) {
  return x * 2;
}, "dbl");
var tri = /* @__PURE__ */ __name((x) => x * 3, "tri");
function quad(x) {
  return x * 4;
}
__name(quad, "quad");
var Shape = class {
  static {
    __name(this, "Shape");
  }
  area() {
    return 1;
  }
};
export {
  Shape,
  dbl,
  quad,
  tri
};
"#;
    let expected = r#"
const dbl = function(x) {
  return x * 2;
};
const tri = (x) => x * 3;
function quad(x) {
  return x * 4;
}
class Shape {
  area() {
    return 1;
  }
}
export { Shape, dbl, quad, tri };
"#;
    assert_eq_normalized(&render(input), expected);
}

// producer esbuild@0.28.0 --bundle --keep-names --minify --format=esm: the
// strings match once the export names replace the mangled bindings.
#[test]
fn pipeline_recovers_minified_esbuild_esm_keep_names_output() {
    let input = r#"var s=Object.defineProperty;var t=(n,r)=>s(n,"name",{value:r,configurable:!0});var i=t(function(n){return n*2},"dbl"),l=t(n=>n*3,"tri");function p(n){return n*4}t(p,"quad");var o=class{static{t(this,"Shape")}area(){return 1}};export{o as Shape,i as dbl,p as quad,l as tri};"#;
    let expected = r#"
export const dbl = function(n) {
  return n * 2;
};
export const tri = (n) => n * 3;
export function quad(n) {
  return n * 4;
}
export class Shape {
  area() {
    return 1;
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}

// producer esbuild@0.28.0 --bundle --keep-names --format=iife: the helpers
// live in the IIFE body.
#[test]
fn pipeline_recovers_esbuild_iife_keep_names_output() {
    let input = r#"
(() => {
  var __defProp = Object.defineProperty;
  var __name = (target, value) => __defProp(target, "name", { value, configurable: true });
  var dbl = /* @__PURE__ */ __name(function(x) {
    return x * 2;
  }, "dbl");
  function quad(x) {
    return x * 4;
  }
  __name(quad, "quad");
  var Shape = class {
    static {
      __name(this, "Shape");
    }
  };
  console.log(dbl(1), quad(1), new Shape());
})();
"#;
    let expected = r#"
(() => {
  var dbl = function(x) {
    return x * 2;
  };
  function quad(x) {
    return x * 4;
  }
  var Shape = class {};
  console.log(dbl(1), quad(1), new Shape());
})();
"#;
    assert_eq_normalized(&render(input), expected);
}

// producer esbuild@0.28.0 --bundle --keep-names --minify --format=iife: no
// export names the mangled bindings, so every wrapper keeps the original
// name.
#[test]
fn pipeline_keeps_wrappers_of_minified_iife_bindings() {
    let input = r#"(()=>{var s=Object.defineProperty;var t=(n,r)=>s(n,"name",{value:r,configurable:!0});var i=t(function(n){return n*2},"dbl");function p(n){return n*4}t(p,"quad");console.log(i(1),p(1))})();"#;
    let output = render(input);
    assert!(output.contains(r#""dbl")"#), "{output}");
    assert!(output.contains(r#""quad")"#), "{output}");
}

// The unpacked module runs the rule after the late recovery's renames.
#[test]
fn unpack_drops_wrappers_in_a_split_module() {
    let input = r#"
var __defProp = Object.defineProperty;
var __name = (target, value) => __defProp(target, "name", { value, configurable: true });
var dbl = /* @__PURE__ */ __name(function(x) {
  return x * 2;
}, "dbl");
function quad(x) {
  return x * 4;
}
__name(quad, "quad");
export {
  dbl,
  quad
};
"#;
    let output = unpack(
        input,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    let all: String = output
        .modules
        .iter()
        .map(|(_, code)| code.as_str())
        .collect();
    assert!(!all.contains("__name"), "{all}");
}

// producer esbuild@0.28.0 --bundle --keep-names --minify --format=iife
// --sourcemap: source-map names give the mangled binding the name the
// wrapper carries, and the rule runs again after them.
#[test]
fn source_map_names_let_minified_wrappers_go() {
    let input = r#"(()=>{var n=Object.defineProperty;var l=(o,c)=>n(o,"name",{value:c,configurable:!0});var s=l(o=>o*2,"dbl");console.log(s(1));})();"#;
    let map = r#"{"version":3,"sources":["in.js"],"sourcesContent":["const dbl = (x) => x * 2;\nconsole.log(dbl(1));\n"],"mappings":"qFAAA,IAAMA,EAAMC,EAACC,GAAMA,EAAI,EAAX,OACZ,QAAQ,IAAIF,EAAI,CAAC,CAAC","names":["dbl","__name","x"]}"#;
    let output = decompile(
        input,
        DecompileOptions {
            filename: "out.js".to_string(),
            sourcemap: Some(map.as_bytes().to_vec()),
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;
    let expected = r#"
(() => {
  var dbl = (x) => x * 2;
  console.log(dbl(1));
})();
"#;
    assert_eq_normalized(&output, expected);
}
