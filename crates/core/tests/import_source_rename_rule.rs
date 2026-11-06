mod common;

use common::{assert_eq_normalized, render, render_rule, render_with_level};
use wakaru_core::driver::test_support::{unpack, unpack_files, UnpackInput};
use wakaru_core::rules::ImportSourceRename;
use wakaru_core::{DecompileOptions, RewriteLevel};

fn apply(input: &str) -> String {
    render_rule(input, ImportSourceRename::new)
}

fn assert_renamed(input: &str, expected: &str) {
    assert_eq_normalized(&apply(input), expected);
}

fn assert_unchanged(input: &str) {
    assert_eq_normalized(&apply(input), input);
}

// ---------------------------------------------------------------------------
// Candidate kinds
// ---------------------------------------------------------------------------

#[test]
fn provisional_module_names_name_default_and_namespace_imports() {
    assert_renamed(
        r#"
import * as Qc from "./module-221.js";
import Ab from "../module-12.js";
Qc.run(Ab);
"#,
        r#"
import * as module221 from "./module-221.js";
import module12 from "../module-12.js";
module221.run(module12);
"#,
    );
}

#[test]
fn a_deduplicated_provisional_name_keeps_its_collision_suffix() {
    assert_renamed(
        r#"
import Ab from "./module-221_2.js";
import * as Cd from "./module-221.js";
Ab(Cd);
"#,
        r#"
import module221_2 from "./module-221_2.js";
import * as module221 from "./module-221.js";
module221_2(module221);
"#,
    );
    // The namespace name builds on the suffixed one.
    assert_renamed(
        r#"
import Ab, * as Cd from "./module-221_2.js";
use(Ab, Cd);
"#,
        r#"
import module221_2, * as module221_2Namespace from "./module-221_2.js";
use(module221_2, module221_2Namespace);
"#,
    );
}

#[test]
fn adjacent_digit_runs_keep_a_separator() {
    assert_renamed(
        r#"
import Ab from "./parts/chunk-12_3.js";
Ab();
"#,
        r#"
import chunk12_3 from "./parts/chunk-12_3.js";
chunk12_3();
"#,
    );
}

#[test]
fn relative_paths_name_the_file_stem() {
    assert_renamed(
        r#"
import Ab from "./shared/utils/date-format.js";
import * as Zt from "../widgets/color_picker.tsx";
Ab(Zt);
"#,
        r#"
import dateFormat from "./shared/utils/date-format.js";
import * as colorPicker from "../widgets/color_picker.tsx";
dateFormat(colorPicker);
"#,
    );
}

#[test]
fn an_index_file_takes_its_directory_name() {
    assert_renamed(
        r#"
import Ab from "./components/toolbar/index.js";
import Cd from "./src/index.js";
Ab(Cd);
"#,
        r#"
import toolbar from "./components/toolbar/index.js";
import Cd from "./src/index.js";
toolbar(Cd);
"#,
    );
}

#[test]
fn taken_names_fall_back_to_parent_prefixes_then_a_suffix() {
    assert_renamed(
        r#"
import Ab from "./shared/utils/date-format.js";
const dateFormat = 1;
Ab(dateFormat);
"#,
        r#"
import utilsDateFormat from "./shared/utils/date-format.js";
const dateFormat = 1;
utilsDateFormat(dateFormat);
"#,
    );
    assert_renamed(
        r#"
import Ab from "./shared/utils/date-format.js";
const dateFormat = 1;
function utilsDateFormat() {}
Ab(dateFormat, utilsDateFormat);
"#,
        r#"
import sharedUtilsDateFormat from "./shared/utils/date-format.js";
const dateFormat = 1;
function utilsDateFormat() {}
sharedUtilsDateFormat(dateFormat, utilsDateFormat);
"#,
    );
    assert_renamed(
        r#"
import Ab from "./src/date-format.js";
const dateFormat = 1;
Ab(dateFormat);
"#,
        r#"
import dateFormat_1 from "./src/date-format.js";
const dateFormat = 1;
dateFormat_1(dateFormat);
"#,
    );
}

#[test]
fn a_taken_provisional_module_name_takes_a_suffix() {
    assert_renamed(
        r#"
import Ab from "./module-12.js";
let module12 = 0;
Ab(module12);
"#,
        r#"
import module12_1 from "./module-12.js";
let module12 = 0;
module12_1(module12);
"#,
    );
}

#[test]
fn packages_name_their_last_meaningful_segment() {
    assert_renamed(
        r#"
import Ab from "lodash/debounce";
import Cd from "lodash.throttle";
import * as Ef from "@tanstack/react-query";
Ab(Cd, Ef);
"#,
        r#"
import debounce from "lodash/debounce";
import lodashThrottle from "lodash.throttle";
import * as reactQuery from "@tanstack/react-query";
debounce(lodashThrottle, reactQuery);
"#,
    );
}

#[test]
fn a_taken_package_segment_joins_its_parents() {
    assert_renamed(
        r#"
import Ab from "lodash/debounce";
function debounce() {}
Ab(debounce);
"#,
        r#"
import lodashDebounce from "lodash/debounce";
function debounce() {}
lodashDebounce(debounce);
"#,
    );
}

#[test]
fn generic_subpaths_start_with_the_joined_name() {
    assert_renamed(
        r#"
import * as Ab from "react-dom/client";
import * as Cd from "@sentry/browser";
import Ef from "some-kit/utils";
Ab(Cd, Ef);
"#,
        r#"
import * as reactDomClient from "react-dom/client";
import * as sentryBrowser from "@sentry/browser";
import someKitUtils from "some-kit/utils";
reactDomClient(sentryBrowser, someKitUtils);
"#,
    );
}

#[test]
fn node_builtins_name_the_module_with_or_without_the_node_prefix() {
    assert_renamed(
        r#"
import * as Ab from "node:path";
import Cd from "child_process";
import * as Ef from "fs/promises";
import Gh from "node:worker_threads";
Ab(Cd, Ef, Gh);
"#,
        r#"
import * as path from "node:path";
import childProcess from "child_process";
import * as fsPromises from "fs/promises";
import workerThreads from "node:worker_threads";
path(childProcess, fsPromises, workerThreads);
"#,
    );
}

#[test]
fn a_builtin_with_and_without_the_node_prefix_is_one_source() {
    assert_renamed(
        r#"
import * as Ab from "fs/promises";
import Cd from "node:fs/promises";
Ab(Cd);
"#,
        r#"
import * as fsPromisesNamespace from "fs/promises";
import fsPromises from "node:fs/promises";
fsPromisesNamespace(fsPromises);
"#,
    );
}

#[test]
fn a_taken_builtin_name_takes_the_node_prefix() {
    assert_renamed(
        r#"
import * as Ab from "path";
var path = "/";
Ab.join(path);
"#,
        r#"
import * as nodePath from "path";
var path = "/";
nodePath.join(path);
"#,
    );
}

#[test]
fn convention_table_spells_community_names() {
    assert_renamed(
        r#"
import Rk from "react";
import * as Rd from "react-dom";
Rd.render(Rk.createElement("div"));
"#,
        r#"
import React from "react";
import * as ReactDOM from "react-dom";
ReactDOM.render(React.createElement("div"));
"#,
    );
}

#[test]
fn invalid_or_reserved_stems_skip_to_the_next_candidate() {
    assert_renamed(
        r#"
import Ab from "./parts/class.js";
import Cd from "./render/3d-utils.js";
import Ef from "./loaders/require.js";
Ab(Cd, Ef);
"#,
        r#"
import partsClass from "./parts/class.js";
import render3dUtils from "./render/3d-utils.js";
import loadersRequire from "./loaders/require.js";
partsClass(render3dUtils, loadersRequire);
"#,
    );
    // No valid candidate at all: the import keeps its name.
    assert_unchanged(
        r#"
import Ab from "./class.js";
import Cd from "./3d-utils.js";
import Ef from "./require.js";
Ab(Cd, Ef);
"#,
    );
}

// ---------------------------------------------------------------------------
// JSX
// ---------------------------------------------------------------------------

#[test]
fn a_jsx_tag_binding_takes_pascal_case_candidates() {
    assert_renamed(
        r#"
import Ab from "./module-12.js";
import Cd from "./shared/date-format.js";
const el = <Ab><Cd /></Ab>;
"#,
        r#"
import Module12 from "./module-12.js";
import DateFormat from "./shared/date-format.js";
const el = <Module12><DateFormat /></Module12>;
"#,
    );
    // Fallbacks stay PascalCase too.
    assert_renamed(
        r#"
import Cd from "./shared/date-format.js";
function DateFormat() {}
const el = <Cd value={DateFormat} />;
"#,
        r#"
import SharedDateFormat from "./shared/date-format.js";
function DateFormat() {}
const el = <SharedDateFormat value={DateFormat} />;
"#,
    );
}

#[test]
fn a_jsx_member_tag_keeps_camel_case() {
    assert_renamed(
        r#"
import * as Ab from "./module-12.js";
const el = <Ab.Panel />;
"#,
        r#"
import * as module12 from "./module-12.js";
const el = <module12.Panel />;
"#,
    );
}

#[test]
fn a_pascal_case_stem_keeps_its_case_whatever_the_use() {
    assert_renamed(
        r#"
import Ab from "./Wrapper.js";
import * as Cd from "./utils/LoginService.js";
var n = require("./widgets/ModalV2.js");
const w = new Ab();
class Session extends Cd.Base {}
n.open(w, Session);
"#,
        r#"
import Wrapper from "./Wrapper.js";
import * as LoginService from "./utils/LoginService.js";
var ModalV2 = require("./widgets/ModalV2.js");
const w = new Wrapper();
class Session extends LoginService.Base {}
ModalV2.open(w, Session);
"#,
    );
    // Prefixed candidates and the namespace name stay PascalCase.
    assert_renamed(
        r#"
import Ab from "./utils/LoginService.js";
import Ef, * as Gh from "./Wrapper.js";
const LoginService = 1;
use(Ab, Ef, Gh, LoginService);
"#,
        r#"
import UtilsLoginService from "./utils/LoginService.js";
import Wrapper, * as WrapperNamespace from "./Wrapper.js";
const LoginService = 1;
use(UtilsLoginService, Wrapper, WrapperNamespace, LoginService);
"#,
    );
    // A readable lowercase export name still comes first.
    assert_renamed(
        r#"
import Ab from "./Wrapper.js";
export { Ab as wrap };
"#,
        r#"
import wrap from "./Wrapper.js";
export { wrap };
"#,
    );
}

#[test]
fn a_leading_capital_decides_the_case() {
    assert_renamed(
        r#"
import Ab from "./Ad.js";
import Cd from "./ads/InLine.js";
import Ef from "./Ab3x9QzK.js";
import Gh from "./wrapper.js";
use(Ab, Cd, Ef, Gh);
"#,
        r#"
import Ad from "./Ad.js";
import InLine from "./ads/InLine.js";
import Ab3x9QzK from "./Ab3x9QzK.js";
import wrapper from "./wrapper.js";
use(Ad, InLine, Ab3x9QzK, wrapper);
"#,
    );
}

// ---------------------------------------------------------------------------
// Collisions
// ---------------------------------------------------------------------------

#[test]
fn an_inner_scope_that_never_reads_the_import_does_not_block_its_name() {
    assert_renamed(
        r#"
import * as X from "node:path";
function a(path) {
  return path.length;
}
X.join(a("x"));
"#,
        r#"
import * as path from "node:path";
function a(path) {
  return path.length;
}
path.join(a("x"));
"#,
    );
}

#[test]
fn an_inner_scope_that_reads_the_import_blocks_its_name() {
    assert_renamed(
        r#"
import * as X from "node:path";
function b(path) {
  return X.join(path);
}
"#,
        r#"
import * as nodePath from "node:path";
function b(path) {
  return nodePath.join(path);
}
"#,
    );
}

#[test]
fn a_nested_var_reaching_the_whole_function_blocks_its_name() {
    assert_renamed(
        r#"
import Ab from "./utils/date-format.js";
function f(c) {
  Ab();
  if (c) {
    var dateFormat = 1;
  }
}
"#,
        r#"
import utilsDateFormat from "./utils/date-format.js";
function f(c) {
  utilsDateFormat();
  if (c) {
    var dateFormat = 1;
  }
}
"#,
    );
}

#[test]
fn a_free_global_reference_blocks_its_name() {
    assert_renamed(
        r#"
import Ab from "node:process";
Ab.exit(process.exitCode);
"#,
        r#"
import nodeProcess from "node:process";
nodeProcess.exit(process.exitCode);
"#,
    );
}

#[test]
fn two_imports_competing_for_one_name_take_distinct_candidates() {
    assert_renamed(
        r#"
import Ab from "./forms/date-format.js";
import Cd from "./tables/date-format.js";
Ab(Cd);
"#,
        r#"
import dateFormat from "./forms/date-format.js";
import tablesDateFormat from "./tables/date-format.js";
dateFormat(tablesDateFormat);
"#,
    );
}

#[test]
fn a_namespace_import_beside_a_default_import_takes_a_namespace_suffix() {
    assert_renamed(
        r#"
import Ab, * as Cd from "./module-12.js";
use(Ab, Cd);
"#,
        r#"
import module12, * as module12Namespace from "./module-12.js";
use(module12, module12Namespace);
"#,
    );
    // Whatever the statement order.
    assert_renamed(
        r#"
import * as Cd from "./module-7.js";
import Ab from "./module-7.js";
use(Ab, Cd);
"#,
        r#"
import * as module7Namespace from "./module-7.js";
import module7 from "./module-7.js";
use(module7, module7Namespace);
"#,
    );
    // A readable default import still marks the namespace import, and the
    // convention spelling is lowered.
    assert_renamed(
        r#"
import React from "react";
import * as Rk from "react";
use(React, Rk);
"#,
        r#"
import React from "react";
import * as reactNamespace from "react";
use(React, reactNamespace);
"#,
    );
    assert_renamed(
        r#"
import dep from "dep";
import * as ns from "dep";
use(dep, ns);
"#,
        r#"
import dep from "dep";
import * as depNamespace from "dep";
use(dep, depNamespace);
"#,
    );
}

#[test]
fn a_taken_namespace_name_takes_a_suffix_not_a_directory_prefix() {
    assert_renamed(
        r#"
import Ab from "./forms/date-format.js";
import * as Cd from "./forms/date-format.js";
const dateFormatNamespace = 1;
use(Ab, Cd, dateFormatNamespace);
"#,
        r#"
import dateFormat from "./forms/date-format.js";
import * as dateFormatNamespace_1 from "./forms/date-format.js";
const dateFormatNamespace = 1;
use(dateFormat, dateFormatNamespace_1, dateFormatNamespace);
"#,
    );
}

#[test]
fn a_namespace_jsx_tag_takes_a_pascal_case_namespace_name() {
    assert_renamed(
        r#"
import Ab from "./module-12.js";
import * as Cd from "./module-12.js";
const el = <Cd value={Ab} />;
"#,
        r#"
import module12 from "./module-12.js";
import * as Module12Namespace from "./module-12.js";
const el = <Module12Namespace value={module12} />;
"#,
    );
}

#[test]
fn two_default_imports_of_one_source_take_the_ordinary_chain() {
    assert_renamed(
        r#"
import Ab from "./forms/date-format.js";
import Cd from "./forms/date-format.js";
use(Ab, Cd);
"#,
        r#"
import dateFormat from "./forms/date-format.js";
import formsDateFormat from "./forms/date-format.js";
use(dateFormat, formsDateFormat);
"#,
    );
    assert_renamed(
        r#"
import Ab from "./module-12.js";
import Cd from "./module-12.js";
use(Ab, Cd);
"#,
        r#"
import module12 from "./module-12.js";
import module12_1 from "./module-12.js";
use(module12, module12_1);
"#,
    );
}

// ---------------------------------------------------------------------------
// Exports, dynamic scope, and what the rule leaves alone
// ---------------------------------------------------------------------------

#[test]
fn exported_import_keeps_its_public_name() {
    // An export under the local's own name keeps it through an alias.
    assert_renamed(
        r#"
import * as Qc from "./module-221.js";
Qc.run();
export { Qc };
"#,
        r#"
import * as module221 from "./module-221.js";
module221.run();
export { module221 as Qc };
"#,
    );
    // A readable export name names the binding better than its source.
    assert_renamed(
        r#"
import * as Qc from "./module-221.js";
Qc.run();
export { Qc as utils };
"#,
        r#"
import * as utils from "./module-221.js";
utils.run();
export { utils };
"#,
    );
    // A generated or default export name is kept with an alias.
    assert_renamed(
        r#"
import * as Qc from "./module-221.js";
import Ab from "./module-12.js";
export { Qc as Zz, Ab as default };
"#,
        r#"
import * as module221 from "./module-221.js";
import module12 from "./module-12.js";
export { module221 as Zz, module12 as default };
"#,
    );
}

#[test]
fn dynamic_scope_keeps_every_name() {
    assert_unchanged(
        r#"
import Ab from "./module-12.js";
eval("Ab()");
"#,
    );
    assert_unchanged(
        r#"
import Ab from "./module-12.js";
function f(o) {
  with (o) {
    Ab();
  }
}
"#,
    );
}

#[test]
fn readable_and_derived_locals_are_left_alone() {
    assert_unchanged(
        r#"
import dateUtils from "./date-format.js";
import * as helpers from "./module-12.js";
import _map from "./module-14.js";
use(dateUtils, helpers, _map);
"#,
    );
}

#[test]
fn named_imports_are_left_alone() {
    assert_unchanged(
        r#"
import { a as b } from "./module-12.js";
import { c } from "./date-format.js";
use(b, c);
"#,
    );
}

#[test]
fn a_member_name_derived_from_the_old_name_follows_it() {
    assert_renamed(
        r#"
import p from "./module-110.js";
const p_version = p.version;
use(p_version);
"#,
        r#"
import module110 from "./module-110.js";
const module110_version = module110.version;
use(module110_version);
"#,
    );
}

// ---------------------------------------------------------------------------
// `require` declarations
// ---------------------------------------------------------------------------

#[test]
fn a_module_scope_require_alias_takes_its_source_name() {
    assert_renamed(
        r#"
var n = require("./module-210.js");
const W = require("./shared/base-codec.js");
n.run(W);
"#,
        r#"
var module210 = require("./module-210.js");
const baseCodec = require("./shared/base-codec.js");
module210.run(baseCodec);
"#,
    );
}

#[test]
fn a_nested_require_alias_takes_its_source_name() {
    assert_renamed(
        r#"
function f() {
  var n = require("./module-210.js");
  return n.run();
}
const g = () => {
  const W = require("node:path");
  return W.join("x");
};
"#,
        r#"
function f() {
  var module210 = require("./module-210.js");
  return module210.run();
}
const g = () => {
  const path = require("node:path");
  return path.join("x");
};
"#,
    );
    // A `var` in a nested block belongs to its function.
    assert_renamed(
        r#"
function f(c) {
  if (c) {
    var n = require("./module-210.js");
    n.run();
  }
}
"#,
        r#"
function f(c) {
  if (c) {
    var module210 = require("./module-210.js");
    module210.run();
  }
}
"#,
    );
}

#[test]
fn requires_that_do_not_hold_their_module_are_left_alone() {
    assert_unchanged(
        r#"
var a = require("./module-210.js").default;
var b = require(name);
var c = require("./module-210.js", extra);
var { d } = require("./module-210.js");
var e = require("./module-211.js");
e = other;
var helpers = require("./module-212.js");
function f(g) {
  var g = require("./module-213.js");
  return g;
}
function h(require) {
  var k = require("./module-214.js");
  return k;
}
use(a, b, c, d, e, helpers);
"#,
    );
    // A `let` or `const` in a nested block is not named.
    assert_unchanged(
        r#"
function f(c) {
  if (c) {
    const n = require("./module-210.js");
    n.run();
  }
}
"#,
    );
}

#[test]
fn a_nested_scope_declaring_the_name_blocks_it() {
    assert_renamed(
        r#"
function f(dateFormat) {
  var n = require("./utils/date-format.js");
  return n(dateFormat);
}
"#,
        r#"
function f(dateFormat) {
  var utilsDateFormat = require("./utils/date-format.js");
  return utilsDateFormat(dateFormat);
}
"#,
    );
}

#[test]
fn an_outer_name_the_scope_reads_blocks_it() {
    assert_renamed(
        r#"
const dateFormat = 1;
function f() {
  var n = require("./utils/date-format.js");
  return n(dateFormat);
}
function g() {
  var W = require("node:process");
  return W.exit(process.exitCode);
}
"#,
        r#"
const dateFormat = 1;
function f() {
  var utilsDateFormat = require("./utils/date-format.js");
  return utilsDateFormat(dateFormat);
}
function g() {
  var nodeProcess = require("node:process");
  return nodeProcess.exit(process.exitCode);
}
"#,
    );
}

#[test]
fn an_outer_name_the_scope_does_not_read_does_not_block_it() {
    assert_renamed(
        r#"
const dateFormat = 1;
function f() {
  var n = require("./utils/date-format.js");
  return n();
}
use(dateFormat);
"#,
        r#"
const dateFormat = 1;
function f() {
  var dateFormat = require("./utils/date-format.js");
  return dateFormat();
}
use(dateFormat);
"#,
    );
}

#[test]
fn an_inner_function_blocks_the_name_only_around_a_read() {
    assert_renamed(
        r#"
function f() {
  var n = require("node:path");
  function g(path) {
    return path.length;
  }
  return n.join(g("x"));
}
function h() {
  var W = require("node:path");
  function k(path) {
    return W.join(path);
  }
  return k;
}
"#,
        r#"
function f() {
  var path = require("node:path");
  function g(path) {
    return path.length;
  }
  return path.join(g("x"));
}
function h() {
  var nodePath = require("node:path");
  function k(path) {
    return nodePath.join(path);
  }
  return k;
}
"#,
    );
}

#[test]
fn a_nested_alias_sees_the_new_names_of_outer_bindings() {
    // The import becomes `dateFormat`, which `f` reads.
    assert_renamed(
        r#"
import Ab from "./utils/date-format.js";
function f() {
  var n = require("./forms/date-format.js");
  return n(Ab);
}
"#,
        r#"
import dateFormat from "./utils/date-format.js";
function f() {
  var formsDateFormat = require("./forms/date-format.js");
  return formsDateFormat(dateFormat);
}
"#,
    );
    // An enclosing scope's alias is named first, whatever the source order.
    assert_renamed(
        r#"
function f() {
  function g() {
    var m = require("./forms/date-format.js");
    return m(n);
  }
  var n = require("./utils/date-format.js");
  return g;
}
"#,
        r#"
function f() {
  function g() {
    var formsDateFormat = require("./forms/date-format.js");
    return formsDateFormat(dateFormat);
  }
  var dateFormat = require("./utils/date-format.js");
  return g;
}
"#,
    );
}

#[test]
fn aliases_of_one_scope_take_distinct_names_and_sibling_scopes_reuse_them() {
    assert_renamed(
        r#"
function f() {
  var a = require("./module-12.js");
  var b = require("./module-12.js");
  return a(b);
}
function g() {
  var c = require("./module-12.js");
  return c;
}
"#,
        r#"
function f() {
  var module12 = require("./module-12.js");
  var module12_1 = require("./module-12.js");
  return module12(module12_1);
}
function g() {
  var module12 = require("./module-12.js");
  return module12;
}
"#,
    );
}

#[test]
fn a_require_alias_never_takes_a_namespace_name() {
    assert_renamed(
        r#"
import Ab from "./module-12.js";
var n = require("./module-12.js");
use(Ab, n);
"#,
        r#"
import module12 from "./module-12.js";
var module12_1 = require("./module-12.js");
use(module12, module12_1);
"#,
    );
}

#[test]
fn a_require_alias_used_as_a_jsx_tag_takes_pascal_case() {
    assert_renamed(
        r#"
var W = require("./shared/date-format.js");
const el = <W />;
"#,
        r#"
var DateFormat = require("./shared/date-format.js");
const el = <DateFormat />;
"#,
    );
}

#[test]
fn an_exported_require_alias_keeps_its_public_name() {
    assert_renamed(
        r#"
var n = require("./module-12.js");
var W = require("./module-14.js");
export { n, W as codec };
"#,
        r#"
var module12 = require("./module-12.js");
var codec = require("./module-14.js");
export { module12 as n, codec };
"#,
    );
    assert_unchanged(
        r#"
export var n = require("./module-12.js");
"#,
    );
}

#[test]
fn a_require_alias_follows_the_dynamic_scope_skip() {
    assert_unchanged(
        r#"
function f() {
  var n = require("./module-12.js");
  return eval("n");
}
"#,
    );
}

// ---------------------------------------------------------------------------
// Full pipeline
// ---------------------------------------------------------------------------

#[test]
fn pipeline_names_converted_requires_after_their_source() {
    let input = r#"
var a = require("./module-221.js");
var b = require("node:path");
function c(path) { return path.length; }
module.exports = function () { return a.run(b.join("x"), c("y")); };
"#;
    let output = render(input);
    assert!(
        output.contains(r#"import module221 from "./module-221.js";"#),
        "{output}"
    );
    assert!(
        output.contains(r#"import path from "node:path";"#),
        "{output}"
    );
    assert!(
        output.contains("module221.run(path.join(\"x\"), c(\"y\"))"),
        "{output}"
    );
}

#[test]
fn pipeline_keeps_the_public_name_of_an_import_exported_under_its_own_name() {
    let input = r#"
import * as Qc from "./module-221.js";
import Ab from "./module-12.js";
Qc.run(Ab);
export { Qc, Ab };
"#;
    let output = render(input);
    assert!(
        output.contains(r#"import * as module221 from "./module-221.js";"#)
            && output.contains(r#"import module12 from "./module-12.js";"#),
        "{output}"
    );
    assert!(
        output.contains("module221 as Qc") && output.contains("module12 as Ab"),
        "{output}"
    );
    assert!(output.contains("module221.run(module12)"), "{output}");
}

#[test]
fn pipeline_keeps_the_collision_suffix_of_a_deduplicated_module() {
    let input = r#"
var a = require("./module-221_2.js");
module.exports = function () { return a.run(); };
"#;
    let output = render(input);
    assert!(
        output.contains(r#"import module221_2 from "./module-221_2.js";"#)
            && output.contains("module221_2.run()"),
        "{output}"
    );
}

#[test]
fn pipeline_names_requires_kept_inside_functions() {
    let input = r#"
var a = require("./module-221.js");
module.exports = function () {
  var b = require("./shared/base-codec.js");
  var c = require("node:path");
  if (a.ready) { return b.encode(c.join("x")); }
  return a.run();
};
"#;
    let output = render(input);
    assert!(
        output.contains(r#"const module221 = require("./module-221.js");"#)
            && output.contains(r#"const baseCodec = require("./shared/base-codec.js");"#)
            && output.contains(r#"const path = require("node:path");"#)
            && output.contains("baseCodec.encode(path.join(\"x\"))")
            && output.contains("module221.run()"),
        "{output}"
    );
    let minimal = render_with_level(input, RewriteLevel::Minimal);
    assert!(!minimal.contains("baseCodec"), "{minimal}");
}

#[test]
fn pipeline_keeps_a_pascal_case_stem_for_a_class_import() {
    let input = r#"
var a = require("./widgets/Wrapper.js");
var b = require("./widgets/panel.js");
module.exports = class extends a { run() { return new a(b); } };
"#;
    let output = render(input);
    assert!(
        output.contains(r#"import Wrapper from "./widgets/Wrapper.js";"#)
            && output.contains(r#"import panel from "./widgets/panel.js";"#)
            && output.contains("extends Wrapper")
            && output.contains("new Wrapper(panel)"),
        "{output}"
    );
}

#[test]
fn pipeline_keeps_names_at_minimal_level() {
    let input = r#"
var a = require("./module-221.js");
module.exports = function () { return a.run(); };
"#;
    let output = render_with_level(input, RewriteLevel::Minimal);
    assert!(!output.contains("module221"), "{output}");
}

#[test]
fn pipeline_value_position_name_outranks_the_source_name() {
    let input = r#"
import r from "./module-12.js";
export default { Toolbar: r };
"#;
    let output = render(input);
    assert!(
        output.contains(r#"import Toolbar from "./module-12.js";"#),
        "{output}"
    );
}

#[test]
fn pipeline_names_a_jsx_component_import_in_pascal_case() {
    let input = r#"
import Ab from "./module-12.js";
import Rk from "react";
export const view = () => Rk.createElement(Ab, null);
"#;
    let output = render(input);
    assert!(output.contains("<Module12/>"), "{output}");
}

// ---------------------------------------------------------------------------
// Unpack
// ---------------------------------------------------------------------------

fn unpack_inputs(inputs: &[(&str, &str)]) -> Vec<(String, String)> {
    unpack_files(
        inputs
            .iter()
            .map(|(filename, source)| UnpackInput {
                filename: filename.to_string(),
                source: source.to_string(),
            })
            .collect(),
        DecompileOptions::default(),
    )
    .expect("inputs should unpack together")
    .modules
}

fn module_code<'a>(modules: &'a [(String, String)], name: &str) -> &'a str {
    modules
        .iter()
        .find(|(filename, _)| filename == name)
        .map(|(_, code)| code.as_str())
        .unwrap_or_else(|| {
            panic!(
                "{name} should exist, got {:?}",
                modules.iter().map(|(name, _)| name).collect::<Vec<_>>()
            )
        })
}

/// Two builds each emit a module 200; the second becomes `module-200_2.js`.
const WIDGET_CHUNK: &str = r#"
(self.webpackChunk_widget = self.webpackChunk_widget || []).push([[9], {
    200: function(module) { module.exports = { run: function () { return "widget"; } }; }
}]);
"#;

const APP_PROVIDER_CHUNK: &str = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[2], {
    200: function(module) { module.exports = { run: function () { return "app"; } }; }
}]);
"#;

#[test]
fn unpacked_consumer_keeps_the_collision_suffix_of_a_deduplicated_provider() {
    let consumer_chunk = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    100: function(module, exports, require) {
        var p = require(200);
        module.exports = function () { return p.run(); };
    }
}]);
"#;
    let modules = unpack_inputs(&[
        ("widget.js", WIDGET_CHUNK),
        ("app-provider.js", APP_PROVIDER_CHUNK),
        ("app.js", consumer_chunk),
    ]);
    let provider = module_code(&modules, "module-200_2.js");
    assert!(provider.contains("\"app\""), "{provider}");
    let consumer = module_code(&modules, "module-100.js");
    assert!(
        consumer.contains(r#"import module200_2 from "./module-200_2.js";"#)
            && consumer.contains("module200_2.run()"),
        "{consumer}"
    );
}

#[test]
fn unpacked_consumer_names_provisional_module_imports() {
    let source = r#"(()=>{var e={1:(e,t,r)=>{e.exports=function(x){return x+1}},2:(e,t,r)=>{var p=r(1);console.log(p(1))}},t={};function r(o){var n=t[o];if(void 0!==n)return n.exports;var s=t[o]={exports:{}};return e[o](s,s.exports,r),s.exports}r(2)})();"#;
    let consumer = |level| {
        let output = unpack(
            source,
            DecompileOptions {
                level,
                ..Default::default()
            },
        )
        .expect("bundle should unpack");
        output
            .modules
            .into_iter()
            .find(|(name, _)| name == "module-2.js")
            .map(|(_, code)| code)
            .expect("consumer should exist")
    };
    let standard = consumer(RewriteLevel::Standard);
    assert!(
        standard.contains(r#"import module1 from "./module-1.js";"#)
            && standard.contains("console.log(module1(1))"),
        "{standard}"
    );
    let minimal = consumer(RewriteLevel::Minimal);
    assert!(!minimal.contains("module1 "), "{minimal}");
}

#[test]
fn unpacked_consumer_names_a_require_kept_inside_a_function() {
    let widget = r#"
(self.webpackChunk_widget = self.webpackChunk_widget || []).push([[9], {
    200: function(module, exports) { exports.run = function () { return "widget"; }; }
}]);
"#;
    let provider = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[2], {
    200: function(module, exports) { exports.run = function () { return "app"; }; }
}]);
"#;
    let consumer_chunk = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    100: function(module, exports, require) {
        module.exports = function () { var p = require(200); return p.run(); };
    }
}]);
"#;
    let modules = unpack_inputs(&[
        ("widget.js", widget),
        ("app-provider.js", provider),
        ("app.js", consumer_chunk),
    ]);
    let consumer = module_code(&modules, "module-100.js");
    assert!(
        consumer.contains(r#"const module200_2 = require("./module-200_2.js");"#)
            && consumer.contains("module200_2.run()"),
        "{consumer}"
    );
}
