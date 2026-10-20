//! The plan unpack builds at barrier 0, where every module has reached
//! `UnEsm` but none has run it, from the require edges of all modules.
//!
//! Requiring a CommonJS module returns its `exports` object, and a consumer
//! may write to it. An ES module has no such object: its namespace is not
//! extensible and its properties cannot be assigned from outside. So a
//! provider that a consumer in the input writes to stays CommonJS, and
//! `UnEsm` reads that decision here instead of guessing alone.
//!
//! A provider counts as written when an edge to it has a write in its uses
//! ([`RequireUses::writes`]): a member assignment, update, or `delete`, the
//! target of an `Object` method that writes, or the argument of a function
//! of the consumer's module that writes that parameter. A value passed to a
//! callee in another module or to an unknown one does not count; a provider
//! recovered as a namespace that meets one gets the `commonjs_exports_escape`
//! warning after the fact barrier instead.
//!
//! A provider whose required value was never a writable CommonJS `exports`
//! object is left alone: a webpack harmony module (its exports are getters
//! `require.d` defined), and a module the input already wrote as ESM.

use crate::collections::{HashMap, HashSet};
use crate::facts::{canonical_module_key, resolve_module_key};
use crate::require_edges::{RequireEdges, RequireUses};

/// One module's input to the plan.
pub(crate) struct PlanModule<'a> {
    pub(crate) filename: &'a str,
    pub(crate) edges: &'a RequireEdges,
    /// Requiring the module returned its CommonJS `exports` object: the
    /// module has no import or export declaration and is not a webpack
    /// harmony module.
    pub(crate) exports_object_writable: bool,
}

/// The decisions barrier 0 makes for every module of an unpack.
#[derive(Debug, Default)]
pub(crate) struct InterfacePlan {
    /// Providers that stay CommonJS, by canonical key, with the first
    /// consumer that writes to each.
    written: HashMap<String, String>,
    /// Modules whose required value was not a writable `exports` object.
    closed: HashSet<String>,
}

impl InterfacePlan {
    pub(crate) fn build<'a>(modules: impl IntoIterator<Item = PlanModule<'a>>) -> Self {
        let modules: Vec<PlanModule<'a>> = modules.into_iter().collect();
        let keys: HashSet<String> = modules
            .iter()
            .map(|module| canonical_module_key(module.filename))
            .collect();
        let closed: HashSet<String> = modules
            .iter()
            .filter(|module| !module.exports_object_writable)
            .map(|module| canonical_module_key(module.filename))
            .collect();
        let mut written: HashMap<String, String> = HashMap::default();
        for module in &modules {
            for edge in module.edges.iter() {
                if !edge.uses.as_ref().is_some_and(RequireUses::writes) {
                    continue;
                }
                let Some(provider) = resolve_module_key(
                    |key| keys.contains(key),
                    Some(module.filename),
                    &edge.source,
                ) else {
                    continue;
                };
                if closed.contains(&provider) {
                    continue;
                }
                written
                    .entry(provider)
                    .or_insert_with(|| module.filename.to_string());
            }
        }
        Self { written, closed }
    }

    /// Whether `UnEsm` keeps the module CommonJS because a consumer in the
    /// input writes to its `exports` object.
    pub(crate) fn keeps_commonjs(&self, filename: &str) -> bool {
        self.written.contains_key(&canonical_module_key(filename))
    }

    /// The first consumer that writes to the module, when one does.
    pub(crate) fn written_by(&self, filename: &str) -> Option<&str> {
        self.written
            .get(&canonical_module_key(filename))
            .map(String::as_str)
    }

    /// Whether requiring the module returned a writable CommonJS `exports`
    /// object.
    pub(crate) fn exports_object_writable(&self, filename: &str) -> bool {
        !self.closed.contains(&canonical_module_key(filename))
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{parse_file_as_module, EsSyntax, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use super::*;
    use crate::rules::transpiler_helper_utils::LocalHelperContext;

    fn edges(source: &str) -> RequireEdges {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(FileName::Anon.into(), source.to_string());
            let mut module = parse_file_as_module(
                &file,
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                None,
                &mut Vec::new(),
            )
            .expect("test source parses");
            let mark = Mark::new();
            module.visit_mut_with(&mut resolver(mark, Mark::new(), false));
            let mut edges = RequireEdges::collect(&module, mark);
            edges.add_uses(
                &module,
                mark,
                &LocalHelperContext::collect_with_mark(&module, mark),
            );
            edges
        })
    }

    /// The plan for one consumer `src/consumer.js` beside the providers
    /// `src/<name>.js`; `closed` names the providers whose required value
    /// was not a writable `exports` object.
    fn plan(consumer: &str, providers: &[&str], closed: &[&str]) -> InterfacePlan {
        let consumer_edges = edges(consumer);
        let empty = RequireEdges::default();
        let names: Vec<String> = providers
            .iter()
            .map(|name| format!("src/{name}.js"))
            .collect();
        let mut modules = vec![PlanModule {
            filename: "src/consumer.js",
            edges: &consumer_edges,
            exports_object_writable: true,
        }];
        for (name, filename) in providers.iter().zip(&names) {
            modules.push(PlanModule {
                filename,
                edges: &empty,
                exports_object_writable: !closed.contains(name),
            });
        }
        InterfacePlan::build(modules)
    }

    #[test]
    fn each_write_form_keeps_its_provider_commonjs() {
        let plan = plan(
            r#"var a = require("./assigned.js");
a.f = 1;
require("./inlined").g = 2;
var u = require("./updated.js");
if (x) { u.count++; }
delete require("./deleted.js").gone;
Object.assign(require("./assign-target.js"), extra);
Object.defineProperty(require("./defined.js"), "k", { get: k });
function install(m) { m.f = 1; }
install(require("./installed.js"));
var r = require("./read.js");
use(r.f);"#,
            &[
                "assigned",
                "inlined",
                "updated",
                "deleted",
                "assign-target",
                "defined",
                "installed",
                "read",
            ],
            &[],
        );
        for name in [
            "assigned",
            "inlined",
            "updated",
            "deleted",
            "assign-target",
            "defined",
            "installed",
        ] {
            let filename = format!("src/{name}.js");
            assert!(plan.keeps_commonjs(&filename), "{name}");
            assert_eq!(plan.written_by(&filename), Some("src/consumer.js"));
        }
        assert!(!plan.keeps_commonjs("src/read.js"));
    }

    #[test]
    fn escapes_that_prove_no_write_leave_the_provider_free() {
        let plan = plan(
            r#"function describe(m) { return Object.keys(m); }
describe(require("./local-reader.js"));
Object.keys(require("./builtin-reader.js"));
var installer = require("./installer.js");
installer.install(require("./cross-module.js"));
register(require("./unknown.js"));
__exportStar(require("./reexported.js"), exports);"#,
            &[
                "local-reader",
                "builtin-reader",
                "installer",
                "cross-module",
                "unknown",
                "reexported",
            ],
            &[],
        );
        for name in [
            "local-reader",
            "builtin-reader",
            "installer",
            "cross-module",
            "unknown",
            "reexported",
        ] {
            assert!(!plan.keeps_commonjs(&format!("src/{name}.js")), "{name}");
        }
    }

    #[test]
    fn a_provider_whose_exports_were_never_writable_is_left_alone() {
        let plan = plan(
            r#"require("./harmony.js").f = 1;
require("./plain.js").f = 1;"#,
            &["harmony", "plain"],
            &["harmony"],
        );
        assert!(!plan.keeps_commonjs("src/harmony.js"));
        assert!(!plan.exports_object_writable("src/harmony.js"));
        assert!(plan.keeps_commonjs("src/plain.js"));
        assert!(plan.exports_object_writable("src/plain.js"));
    }

    #[test]
    fn a_source_outside_the_input_is_not_planned() {
        let plan = plan(r#"require("lodash").f = 1;"#, &[], &[]);
        assert!(plan.written.is_empty());
    }
}
