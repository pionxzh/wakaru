mod common;

use common::{render, render_pipeline_between_with_facts, render_with_level};
use wakaru_core::facts::ModuleFactsMap;
use wakaru_core::RewriteLevel;

#[test]
fn babel_named_import_becomes_namespace_and_default_import_stays() {
    // `_utils` is a default import: Babel reads it through
    // `_interopRequireDefault(...).default`, which `UnInteropRequireDefault`
    // later rewrites to the same shape as the named import `_dep`.
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.run = run;
var _utils = _interopRequireDefault(require("./utils"));
var _dep = require("./dep");
function _interopRequireDefault(e) { return e && e.__esModule ? e : { default: e }; }
function run() { return [_utils.default.format(), _dep.value]; }
"#;
    let output = render(input);
    assert!(
        output.contains(r#"import _utils from "./utils";"#),
        "{output}"
    );
    assert!(
        output.contains(r#"import * as _dep from "./dep";"#),
        "{output}"
    );
}

#[test]
fn typescript_named_import_becomes_namespace() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.readValue = readValue;
const dep_js_1 = require("./dep.js");
function readValue() { return dep_js_1.value; }
"#;
    let output = render(input);
    assert!(
        output.contains(r#"import * as dep_js_1 from "./dep.js";"#),
        "{output}"
    );
}

#[test]
fn esbuild_named_import_becomes_namespace() {
    // esbuild has no `__esModule` marker; `__toCommonJS` is the evidence.
    let input = r#"
var __defProp = Object.defineProperty;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, { get: all[name], enumerable: true });
};
var __copyProps = (to, from, except, desc) => {
  if (from && typeof from === "object" || typeof from === "function") {
    for (let key of __getOwnPropNames(from))
      if (!__hasOwnProp.call(to, key) && key !== except)
        __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
  }
  return to;
};
var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);
var mod_exports = {};
__export(mod_exports, {
  readValue: () => readValue
});
module.exports = __toCommonJS(mod_exports);
var import_dep = require("./dep.js");
function readValue() {
  return import_dep.value;
}
"#;
    let output = render(input);
    assert!(
        output.contains(r#"import * as import_dep from "./dep.js";"#),
        "{output}"
    );
}

#[test]
fn sucrase_binding_wrapped_by_wildcard_interop_becomes_namespace() {
    let input = r#"
"use strict";Object.defineProperty(exports, "__esModule", {value: true}); function _interopRequireWildcard(obj) { if (obj && obj.__esModule) { return obj; } else { var newObj = {}; if (obj != null) { for (var key in obj) { if (Object.prototype.hasOwnProperty.call(obj, key)) { newObj[key] = obj[key]; } } } newObj.default = obj; return newObj; } }
var _depjs = require('./dep.js'); var all = _interopRequireWildcard(_depjs);
function read() { return [_depjs.value, all.value]; } exports.read = read;
"#;
    let output = render(input);
    assert!(output.contains("import * as _depjs from"), "{output}");
    assert!(!output.contains("import _depjs from"), "{output}");
}

#[test]
fn binding_passed_to_interop_default_helper_stays_default() {
    // sucrase wraps a separate binding for a default import.
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
function _interopRequireDefault(obj) { return obj && obj.__esModule ? obj : { default: obj }; }
var _dep = require("./dep");
var _dep2 = _interopRequireDefault(_dep);
function read() { return [_dep.value, _dep2.default.format()]; }
exports.read = read;
"#;
    let output = render(input);
    assert!(!output.contains("import * as"), "{output}");
}

#[test]
fn default_member_read_stays_default() {
    // TypeScript without esModuleInterop reads a default import as `.default`.
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const dep_1 = require("./dep");
exports.read = () => [dep_1.default, dep_1.value];
"#;
    let output = render(input);
    assert!(output.contains(r#"import dep_1 from "./dep";"#), "{output}");
}

#[test]
fn module_without_esm_evidence_stays_default() {
    let input = r#"
var dep = require("./dep");
exports.read = () => dep.value;
"#;
    let output = render(input);
    assert!(output.contains(r#"import dep from "./dep";"#), "{output}");
}

#[test]
fn bare_specifier_stays_default() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const pkg_1 = require("pkg");
exports.read = () => pkg_1.value;
"#;
    let output = render(input);
    assert!(output.contains(r#"import pkg_1 from "pkg";"#), "{output}");
}

#[test]
fn whole_value_use_stays_default() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const dep_1 = require("./dep");
exports.read = () => [dep_1.value, dep_1];
"#;
    let output = render(input);
    assert!(output.contains(r#"import dep_1 from "./dep";"#), "{output}");
}

#[test]
fn minimal_level_stays_default() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const dep_1 = require("./dep");
exports.read = () => dep_1.value;
"#;
    let output = render_with_level(input, RewriteLevel::Minimal);
    assert!(!output.contains("import * as"), "{output}");
}

#[test]
fn provider_facts_leave_the_decision_to_the_fact_repair() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const dep_1 = require("./dep");
exports.read = () => dep_1.value;
"#;
    let facts = ModuleFactsMap::new();
    let output = render_pipeline_between_with_facts(
        input,
        "UnInteropRequireDefault",
        "RelativeNamespaceImport",
        &facts,
        Some("fixture.js"),
    );
    assert!(output.contains(r#"import dep_1 from "./dep";"#), "{output}");
}
