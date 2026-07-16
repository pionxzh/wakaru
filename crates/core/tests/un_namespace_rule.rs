mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_core::rules::UnNamespace;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnNamespace)
}

#[test]
fn flattens_typescript_namespace_iife_to_stable_alias_block() {
    // TypeScript 5.x output for an exported namespace value, as reproduced by
    // Zod's `objectUtil.mergeShapes` namespace.
    let input = r#"
let objectUtil;
(function (namespace) {
  namespace.mergeShapes = (first, second) => ({ ...first, ...second });
})(objectUtil || (objectUtil = {}));
"#;
    let expected = r#"
let objectUtil;
{
  const namespace = objectUtil || (objectUtil = {});
  namespace.mergeShapes = (first, second) => ({ ...first, ...second });
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn flattens_arrow_namespace_iife() {
    let input = r#"
let objectUtil;
((namespace) => {
  namespace.mergeShapes = (first, second) => ({ ...first, ...second });
})(objectUtil || (objectUtil = {}));
"#;
    let expected = r#"
let objectUtil;
{
  const namespace = objectUtil || (objectUtil = {});
  namespace.mergeShapes = (first, second) => ({ ...first, ...second });
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_sequential_namespace_assignments_on_one_alias() {
    let input = r#"
let helpers;
(function (namespace) {
  namespace.first = makeFirst();
  namespace.second = makeSecond();
})(helpers || (helpers = {}));
"#;
    let expected = r#"
let helpers;
{
  const namespace = helpers || (helpers = {});
  namespace.first = makeFirst();
  namespace.second = makeSecond();
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_multiple_namespace_augmentations_separate() {
    let input = r#"
let helpers;
(function (namespace) {
  namespace.first = makeFirst();
})(helpers || (helpers = {}));
(function (namespace) {
  namespace.second = makeSecond();
})(helpers || (helpers = {}));
"#;
    let expected = r#"
let helpers;
{
  const namespace = helpers || (helpers = {});
  namespace.first = makeFirst();
}
{
  const namespace = helpers || (helpers = {});
  namespace.second = makeSecond();
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn allows_nested_function_own_this_and_arguments() {
    let input = r#"
let helpers;
(function (namespace) {
  namespace.read = function () { return this.value + arguments.length; };
})(helpers || (helpers = {}));
"#;
    let expected = r#"
let helpers;
{
  const namespace = helpers || (helpers = {});
  namespace.read = function () { return this.value + arguments.length; };
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_namespace_iife_with_private_declaration() {
    let input = r#"
let helpers;
(function (namespace) {
  function privateHelper() { return 1; }
  namespace.read = privateHelper;
})(helpers || (helpers = {}));
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_namespace_iife_with_direct_eval() {
    let input = r#"
let helpers;
(function (namespace) {
  namespace.read = eval(source);
})(helpers || (helpers = {}));
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_namespace_iife_with_nested_direct_eval() {
    let input = r#"
let helpers;
(function (namespace) {
  namespace.read = function () { eval("namespace = other"); };
})(helpers || (helpers = {}));
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_namespace_iife_with_lexical_this_capture() {
    let input = r#"
let helpers;
(function (namespace) {
  namespace.read = () => this.value;
})(helpers || (helpers = {}));
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_namespace_iife_with_lexical_arguments_capture() {
    let input = r#"
let helpers;
(function (namespace) {
  namespace.read = () => arguments[0];
})(helpers || (helpers = {}));
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_namespace_iife_with_new_target_capture() {
    let input = r#"
function build() {
  let helpers;
  (function (namespace) {
    namespace.read = () => new.target;
  })(helpers || (helpers = {}));
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_namespace_iife_when_alias_can_be_reassigned() {
    let input = r#"
let helpers;
(function (namespace) {
  namespace.read = () => { namespace = other; };
})(helpers || (helpers = {}));
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_non_namespace_iife_argument() {
    let input = r#"
let helpers;
(function (namespace) {
  namespace.read = makeReader();
})(helpers);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn pipeline_keeps_string_enum_object_recovery() {
    let input = r#"
var Status;
(function (Status) {
  Status.Ready = "ready";
  Status.Done = "done";
})(Status || (Status = {}));
"#;
    let expected = r#"
const Status = {
  Ready: "ready",
  Done: "done"
};
"#;
    assert_eq_normalized(&render_pipeline(input), expected);
}

#[test]
fn pipeline_recovers_non_literal_namespace_as_alias_block() {
    let input = r#"
var objectUtil;
(function (namespace) {
  namespace.mergeShapes = (first, second) => ({ ...first, ...second });
})(objectUtil || (objectUtil = {}));
use(objectUtil);
"#;
    let expected = r#"
let objectUtil;
{
  const namespace = objectUtil || (objectUtil = {});
  namespace.mergeShapes = (first, second) => ({ ...first, ...second });
}
use(objectUtil);
"#;
    assert_eq_normalized(&render_pipeline(input), expected);
}
