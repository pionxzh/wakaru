mod common;

use common::{assert_eq_normalized, render_pipeline, render_pipeline_until};
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{BindingIdent, Decl, EsVersion, ModuleItem, Pat, Stmt};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_core::rules::UnEnum;

fn apply(input: &str) -> String {
    apply_rule(input, false)
}

fn apply_resolved(input: &str) -> String {
    apply_rule(input, true)
}

fn apply_rule(input: &str, resolve_bindings: bool) -> String {
    GLOBALS.set(&Default::default(), || {
        use swc_core::common::{sync::Lrc, FileName, SourceMap};
        use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
        use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("test.js".to_string()).into(),
            input.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax {
                jsx: true,
                ..Default::default()
            }),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let mut module = parser.parse_module().expect("parse failed");

        if resolve_bindings {
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
            module.visit_mut_with(&mut UnEnum::new_with_mark(unresolved_mark));
        } else {
            module.visit_mut_with(&mut UnEnum::new());
        }

        let mut output = Vec::new();
        {
            let mut emitter = Emitter {
                cfg: Config::default().with_minify(false),
                cm: cm.clone(),
                comments: None,
                wr: JsWriter::new(cm, "\n", &mut output, None),
            };
            emitter.emit_module(&module).expect("emit failed");
        }
        String::from_utf8(output).expect("utf-8")
    })
}

#[test]
fn recovers_exported_commonjs_enum() {
    let input = r#"
var LocalMode;
(function (e) {
  e[e["Dev"] = 0] = "Dev";
  e["Prod"] = "prod";
})(LocalMode = exports.Mode || (exports.Mode = {}));
"#;
    let expected = r#"
var LocalMode = {
  Dev: 0,
  0: "Dev",
  Prod: "prod"
};
export { LocalMode as Mode };
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn recovers_collapsed_exported_commonjs_enum() {
    let input = r#"
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
"#;
    let expected = r#"
export var Mode = {
  Dev: "dev"
};
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn recovers_collapsed_exported_numeric_enum() {
    let input = r#"
(function (e) {
  e[e.Dev = 0] = "Dev";
})(exports.Mode || (exports.Mode = {}));
"#;
    let expected = r#"
export var Mode = {
  Dev: 0,
  0: "Dev"
};
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn recovers_collapsed_exported_enum_mangled_param() {
    let input = r#"
(function (t) {
  t["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
"#;
    let expected = r#"
export var Mode = {
  Dev: "dev"
};
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn collapsed_exported_enum_rejects_later_public_export_read() {
    let input = r#"
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
observe(exports.Mode);
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_rejects_existing_local_name() {
    let input = r#"
var Mode = 1;
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_rejects_import_collision() {
    let input = r#"
import { Mode } from "./dep.js";
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
use(Mode);
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_rejects_global_reference() {
    // `observe(Mode)` reads a global; a synthesized `var Mode` would
    // capture it (and throw via TDZ once promoted to `const`).
    let input = r#"
observe(Mode);
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_rejects_reserved_name() {
    // `exports.class` is a legal property, but the synthesized binding
    // `export var class` would be a SyntaxError.
    let input = r#"
(function (e) {
  e["Dev"] = "dev";
})(exports.class || (exports.class = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_rejects_earlier_public_export_read() {
    // A function defined before the IIFE can defer its `exports.Mode` read
    // until after the fold removed the only write.
    let input = r#"
function readMode() {
  return exports.Mode;
}
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
observe(readMode());
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_enum_rejects_earlier_public_export_read() {
    // Same deferred-read hazard for the local-alias form: the fold removes
    // the `exports.Mode` write that `readMode` observes.
    let input = r#"
function readMode() {
  return exports.Mode;
}
var LocalMode;
(function (e) {
  e["Dev"] = "dev";
})(LocalMode = exports.Mode || (exports.Mode = {}));
observe(readMode());
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_rejects_direct_eval_mentioning_name() {
    // Direct eval resolves names lexically; the synthesized module binding
    // would capture what used to be a global lookup.
    let input = r#"
observe(eval("Mode"));
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_rejects_unknown_direct_eval() {
    let input = r#"
observe(eval(source));
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_allows_unrelated_direct_eval() {
    let input = r#"
observe(eval("1 + 1"));
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
"#;
    let expected = r#"
observe(eval("1 + 1"));
export var Mode = {
  Dev: "dev"
};
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn collapsed_exported_enum_rejects_invalid_identifier_name() {
    // `a\u{B2}` (a²) passes a naive alphanumeric check but is not valid
    // JavaScript identifier grammar; the synthesized binding would be a
    // SyntaxError.
    let input = "
(function (e) {
  e[\"Dev\"] = \"dev\";
})(exports[\"a\u{B2}\"] || (exports[\"a\u{B2}\"] = {}));
";
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_rejects_exports_escape() {
    // The exports object escapes as a value; `publicApi.Mode` observes the
    // write the fold would remove.
    let input = r#"
const publicApi = exports;
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
observe(publicApi.Mode);
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_rejects_dynamic_exports_access() {
    let input = r#"
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
observe(exports[key]);
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_rejects_module_exports_read() {
    let input = r#"
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
observe(module.exports.Mode);
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn collapsed_exported_enum_allows_unrelated_static_export_write() {
    let input = r#"
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
exports.other = 1;
"#;
    let expected = r#"
export var Mode = {
  Dev: "dev"
};
exports.other = 1;
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn collapsed_exported_enum_allows_indirect_eval() {
    // Indirect eval runs in global scope and cannot observe module
    // bindings, so it does not block synthesis.
    let input = r#"
observe((0, eval)("Mode"));
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
"#;
    let expected = r#"
observe((0, eval)("Mode"));
export var Mode = {
  Dev: "dev"
};
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn enum_member_key_with_invalid_identifier_grammar_stays_string() {
    // `a\u{B2}` passes a naive alphanumeric check; an ident key would emit
    // invalid JavaScript.
    let input = "
var Mode;
(function (e) {
  e[\"a\u{B2}\"] = \"x\";
})(Mode || (Mode = {}));
";
    let expected = "
var Mode = {
  \"a\u{B2}\": \"x\"
};
";
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn collapsed_exported_enum_rejects_prior_bare_var() {
    // The collapsed form never assigns the local, so `var Mode` must stay
    // `undefined` — folding into it would change what later reads observe.
    let input = r#"
var Mode;
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
use(Mode);
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn pipeline_promotes_adjacent_exported_enum_to_const() {
    let input = r#"
export let Mode;
(function (e) {
  e["Dev"] = "dev";
})(Mode || (Mode = {}));
use(Mode);
"#;
    let expected = r#"
export const Mode = {
  Dev: "dev"
};
use(Mode);
"#;
    assert_eq_normalized(&render_pipeline(input), expected);
}

#[test]
fn pipeline_recovers_minified_cjs_enum() {
    let input = r#"
exports.Mode = void 0;
(function (e) {
  e["Dev"] = "dev";
})(exports.Mode || (exports.Mode = {}));
"#;
    let expected = r#"
export const Mode = {
  Dev: "dev"
};
"#;
    assert_eq_normalized(&render_pipeline(input), expected);
}

#[test]
fn pipeline_recovers_minified_cjs_numeric_enum() {
    let input = r#"
exports.Mode = void 0;
(function (e) {
  e[e.Dev = 0] = "Dev";
  e[e.Prod = 1] = "Prod";
})(exports.Mode || (exports.Mode = {}));
"#;
    let expected = r#"
export const Mode = {
  Dev: 0,
  0: "Dev",
  Prod: 1,
  1: "Prod"
};
"#;
    assert_eq_normalized(&render_pipeline(input), expected);
}

#[test]
fn pipeline_still_recovers_minified_cjs_enum_with_local_alias() {
    // Same producer, but module-scope compress kept the local alias as a
    // mangled name instead of collapsing it. This form already recovered
    // before collapsed-form support; it must keep working.
    let input = r#"
exports.Mode = void 0;
var r;
(function (t) {
  t["Dev"] = "dev";
  t["Prod"] = "prod";
})(r = exports.Mode || (exports.Mode = {}));
"#;
    let expected = r#"
export const Mode = {
  Dev: "dev",
  Prod: "prod"
};
"#;
    assert_eq_normalized(&render_pipeline(input), expected);
}

#[test]
fn recovers_alternate_exported_commonjs_enum_form() {
    let input = r#"
var Mode;
(function (e) {
  e[e["Dev"] = -1] = "Dev";
})(Mode || (exports.Mode = Mode = {}));
"#;
    let expected = r#"
var Mode = {
  Dev: -1,
  [-1]: "Dev"
};
export { Mode };
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn exported_commonjs_enum_preserves_position_after_split_declarations() {
    let input = r#"
var Mode;
var before = "initialized first";
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Mode || (exports.Mode = {}));
"#;
    let expected = r#"
var Mode;
var before = "initialized first";
Mode = {
  Dev: 0,
  0: "Dev"
};
export { Mode };
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn exported_commonjs_enum_rejects_intervening_binding_use() {
    let input = r#"
var Mode;
var before = observe(Mode);
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Mode || (exports.Mode = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_rejects_intervening_public_export_use() {
    let input = r#"
var Mode;
observe(exports.Mode);
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Mode || (exports.Mode = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_rejects_later_public_export_read() {
    let input = r#"
var Mode;
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Mode || (exports.Mode = {}));
observe(exports.Mode);
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn split_exported_commonjs_enum_rejects_later_deferred_public_read() {
    let input = r#"
var Mode;
prepare();
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode || (exports.Mode = Mode = {}));
function later() {
  return exports.Mode;
}
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_allows_later_other_public_export_read() {
    let input = r#"
var Mode;
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Mode || (exports.Mode = {}));
observe(exports.Other);
"#;
    let expected = r#"
var Mode = {
  Dev: 0,
  0: "Dev"
};
export { Mode };
observe(exports.Other);
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn exported_commonjs_enum_rejects_effectful_values() {
    let input = r#"
var Mode;
(function (e) {
  e[e["Dev"] = observe()] = "Dev";
})(Mode = exports.Mode || (exports.Mode = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_rejects_local_exports_binding() {
    let input = r#"
const exports = {};
var Mode;
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Mode || (exports.Mode = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_rejects_mismatched_public_names() {
    let input = r#"
var Mode;
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Mode || (exports.Other = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_rejects_duplicate_esm_export() {
    let input = r#"
var Existing;
export { Existing as Mode };
var Mode;
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Mode || (exports.Mode = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_rejects_duplicate_exported_function_name() {
    let input = r#"
export function Existing() {}
var Mode;
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Existing || (exports.Existing = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_rejects_duplicate_exported_class_name() {
    let input = r#"
export class Existing {}
var Mode;
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Existing || (exports.Existing = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_rejects_duplicate_destructured_export_name() {
    let input = r#"
export var { Existing } = source;
var Mode;
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Existing || (exports.Existing = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_rejects_duplicate_namespace_export_name() {
    let input = r#"
export * as Existing from "./dep.js";
var Mode;
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Existing || (exports.Existing = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_rejects_duplicate_string_export_name() {
    let input = r#"
const value = 1;
export { value as "Existing" };
var Mode;
(function (e) {
  e[e["Dev"] = 0] = "Dev";
})(Mode = exports.Existing || (exports.Existing = {}));
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn exported_commonjs_enum_is_top_level_only() {
    let input = r#"
function make() {
  var Mode;
  (function (e) {
    e[e["Dev"] = 0] = "Dev";
  })(Mode = exports.Mode || (exports.Mode = {}));
}
"#;
    assert_eq_normalized(&apply_resolved(input), input);
}

#[test]
fn test_numeric_enum() {
    let input = r#"
var Direction;
(function (Direction) {
  Direction[Direction["Up"] = 1] = "Up";
  Direction[Direction["Down"] = 2] = "Down";
  Direction[Direction["Left"] = 3] = "Left";
  Direction[Direction["Right"] = -4] = "Right";
})(Direction || (Direction = {}));
"#;
    let expected = r#"
var Direction = {
  Up: 1,
  1: "Up",
  Down: 2,
  2: "Down",
  Left: 3,
  3: "Left",
  Right: -4,
  [-4]: "Right"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn numeric_enum_preserves_forward_reverse_overwrite_order() {
    let input = r#"
var Direction;
(function (Direction) {
  Direction[Direction["A"] = 1] = "A";
  Direction[Direction["1"] = 2] = "1";
})(Direction || (Direction = {}));
"#;
    let expected = r#"
var Direction = {
  A: 1,
  1: "A",
  "1": 2,
  2: "1"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_enum_with_non_literal_member_value() {
    let input = r#"
var Direction;
(function (Direction) {
  Direction[Direction["Up"] = nextValue()] = "Up";
})(Direction || (Direction = {}));
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn test_string_enum() {
    let input = r#"
var Direction;
(function (Direction) {
  Direction["Up"] = "UP";
  Direction["Down"] = "DOWN";
  Direction.Left = "LEFT";
  Direction.Right = "RIGHT";
})(Direction || (Direction = {}));
"#;
    let expected = r#"
var Direction = {
  Up: "UP",
  Down: "DOWN",
  Left: "LEFT",
  Right: "RIGHT"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_mangled_enum() {
    let input = r#"
var Direction;
(function (i) {
  i[i["Up"] = 1] = "Up";
  i[i["Down"] = 2] = "Down";
})(Direction || (Direction = {}));
"#;
    let expected = r#"
var Direction = {
  Up: 1,
  1: "Up",
  Down: 2,
  2: "Down"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_function_initializer_enum() {
    let input = r#"
var Direction = function (Direction) {
  Direction[Direction["Up"] = 1] = "Up";
  Direction[Direction["Down"] = 2] = "Down";
  return Direction;
}(Direction || {});
"#;
    let expected = r#"
var Direction = {
  Up: 1,
  1: "Up",
  Down: 2,
  2: "Down"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_arrow_initializer_enum() {
    let input = r#"
var Direction = ((Direction2) => {
  Direction2[Direction2["Up"] = 1] = "Up";
  Direction2[Direction2["Down"] = 2] = "Down";
  return Direction2;
})(Direction || {});
"#;
    let expected = r#"
var Direction = {
  Up: 1,
  1: "Up",
  Down: 2,
  2: "Down"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_concise_arrow_sequence_initializer_enum() {
    let input = r#"
var Direction = (Direction2 => (
  Direction2[Direction2.Up = 1] = "Up",
  Direction2[Direction2.Down = 2] = "Down",
  Direction2[Direction2.Right = -4] = "Right",
  Direction2
))(Direction || {});
"#;
    let expected = r#"
var Direction = {
  Up: 1,
  1: "Up",
  Down: 2,
  2: "Down",
  Right: -4,
  [-4]: "Right"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn pipeline_recovers_sequence_return_enum_after_conditionals() {
    let input = r#"
var Direction=(Direction2=>(Direction2[Direction2.Up=1]="Up",Direction2[Direction2.Down=2]="Down",Direction2[Direction2.Left=4]="Left",Direction2[Direction2.Right=-4]="Right",Direction2))(Direction||{});use(1,Direction[2],-4);
"#;
    let expected = r#"
var Direction = {
  Up: 1,
  1: "Up",
  Down: 2,
  2: "Down",
  Left: 4,
  4: "Left",
  Right: -4,
  [-4]: "Right"
};
use(1, Direction[2], -4);
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEnum"), expected);
}

#[test]
fn test_concise_arrow_sequence_string_enum() {
    let input = r#"
var Status = (Status2 => (
  Status2.Ready = "ready",
  Status2.Done = "done",
  Status2
))(Status || {});
"#;
    let expected = r#"
var Status = {
  Ready: "ready",
  Done: "done"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_exported_function_initializer_enum() {
    let input = r#"
export let Mode = function (Mode) {
  Mode["Dev"] = "dev";
  Mode["Prod"] = "prod";
  return Mode;
}({});
"#;
    let expected = r#"
export let Mode = {
  Dev: "dev",
  Prod: "prod"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_enum_invalid_identifier_keys() {
    let input = r#"
var RenderMode;
(function (RenderMode) {
  RenderMode[RenderMode["2D"] = 1] = "2D";
  RenderMode[RenderMode["WebGL"] = 2] = "WebGL";
})(RenderMode || (RenderMode = {}));
"#;
    let expected = r#"
var RenderMode = {
  "2D": 1,
  1: "2D",
  WebGL: 2,
  2: "WebGL"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_noop_not_an_enum() {
    let input = r#"
var x = 1;
console.log(x);
"#;
    let expected = r#"
var x = 1;
console.log(x);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn bare_var_enum_preserves_binding_context() {
    let input = r#"
var Direction;
(function(Direction) {
    Direction[Direction["Up"] = 0] = "Up";
    Direction[Direction["Down"] = 1] = "Down";
})(Direction || (Direction = {}));
console.log(Direction.Up);
"#;

    let (original_ctxt, output_ctxt) = enum_var_binding_context(input);

    assert_eq!(
        output_ctxt, original_ctxt,
        "enum var declaration should preserve the original binding context"
    );
    assert_ne!(
        output_ctxt,
        SyntaxContext::empty(),
        "regression input should use a scoped binding"
    );
}

#[test]
fn standalone_enum_preserves_binding_context() {
    let input = r#"
(function(Status) {
    Status[Status["Active"] = 0] = "Active";
    Status[Status["Inactive"] = 1] = "Inactive";
})(Status || (Status = {}));
console.log(Status.Active);
"#;

    let (original_ctxt, output_ctxt) = standalone_enum_binding_context(input);

    assert_eq!(
        output_ctxt, original_ctxt,
        "standalone enum assignment should preserve the original binding context"
    );
    assert_ne!(
        output_ctxt,
        SyntaxContext::empty(),
        "regression input should use a scoped binding"
    );
}

fn enum_var_binding_context(input: &str) -> (SyntaxContext, SyntaxContext) {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("fixture.js".to_string()).into(),
            input.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax::default()),
            EsVersion::latest(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let mut module = parser.parse_module().expect("input should parse");

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        let original_ctxt = first_var_decl_context(&module);

        module.visit_mut_with(&mut UnEnum::new_with_mark(unresolved_mark));

        let output_ctxt = first_var_decl_context(&module);

        (original_ctxt, output_ctxt)
    })
}

fn standalone_enum_binding_context(input: &str) -> (SyntaxContext, SyntaxContext) {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("fixture.js".to_string()).into(),
            input.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax::default()),
            EsVersion::latest(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let mut module = parser.parse_module().expect("input should parse");

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        let original_ctxt = iife_arg_ident_context(&module);

        module.visit_mut_with(&mut UnEnum::new_with_mark(unresolved_mark));

        let output_ctxt = first_assign_target_context(&module);

        (original_ctxt, output_ctxt)
    })
}

fn first_var_decl_context(module: &swc_core::ecma::ast::Module) -> SyntaxContext {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &module.body[0] else {
        panic!("expected var declaration as first statement");
    };
    let Pat::Ident(BindingIdent { id, .. }) = &var.decls[0].name else {
        panic!("expected identifier in var declaration");
    };
    id.ctxt
}

fn iife_arg_ident_context(module: &swc_core::ecma::ast::Module) -> SyntaxContext {
    use swc_core::ecma::ast::{CallExpr, Expr, ExprStmt};
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &module.body[0] else {
        panic!("expected expression statement");
    };
    let Expr::Call(CallExpr { args, .. }) = expr.as_ref() else {
        panic!("expected call expression");
    };
    let arg = strip_parens_ref(&args[0].expr);
    let Expr::Bin(bin) = arg else {
        panic!("expected binary expression in arg");
    };
    let Expr::Ident(id) = bin.left.as_ref() else {
        panic!("expected ident in left of LogicalOr");
    };
    id.ctxt
}

fn strip_parens_ref(expr: &swc_core::ecma::ast::Expr) -> &swc_core::ecma::ast::Expr {
    use swc_core::ecma::ast::Expr;
    let mut current = expr;
    while let Expr::Paren(p) = current {
        current = p.expr.as_ref();
    }
    current
}

fn first_assign_target_context(module: &swc_core::ecma::ast::Module) -> SyntaxContext {
    use swc_core::ecma::ast::{AssignExpr, AssignTarget, Expr, ExprStmt, SimpleAssignTarget};
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &module.body[0] else {
        panic!("expected expression statement after transformation");
    };
    let Expr::Assign(AssignExpr { left, .. }) = expr.as_ref() else {
        panic!("expected assignment expression");
    };
    let AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent { id, .. })) = left else {
        panic!("expected identifier assign target");
    };
    id.ctxt
}
