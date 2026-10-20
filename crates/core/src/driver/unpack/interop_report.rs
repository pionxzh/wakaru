//! `wakaru debug interop`: each module's provider interface and each of its
//! `require()` edges, read where unpack makes its cross-module decisions.

use std::sync::Mutex;

use anyhow::Result;
use swc_core::ecma::ast::{ImportSpecifier, Module, ModuleDecl, ModuleExportName, ModuleItem};

use super::super::types::DecompileOptions;
use super::{
    nested_scope_split_enabled, prepare_unpack_input_with_policy, unpack_prepared_inputs_impl,
    ScopeHoistPolicy,
};
use crate::analysis::binding_id;
use crate::collections::HashMap;
use crate::facts::ModuleFacts;
use crate::interface_plan::InterfacePlan;
use crate::provider_interface::{ProviderInterface, RequiredValueImport};
use crate::require_edges::{EdgeKey, ImportTags, RequireEdge, RequireForm};
use crate::rules::transpiler_helper_utils::TranspilerHelperKind;

/// Every module of an unpack, with its provider interface and require edges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InteropReport {
    pub modules: Vec<ModuleInterop>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleInterop {
    pub filename: String,
    /// What `require` of this module returned while it was CommonJS, and the
    /// import that reads that value.
    pub provider: ProviderReport,
    /// The module has no import or export declaration after the passes at
    /// the fact barrier.
    pub stays_commonjs: bool,
    /// The module stayed CommonJS because a consumer in the input writes to
    /// its `exports` object: the first such consumer.
    pub written_by: Option<String>,
    /// The interop helpers the module exports, as barrier 0 proved them:
    /// `name (default interop)` or `name (wildcard interop)`, with
    /// `module.exports` naming the required value itself.
    pub helper_exports: Vec<String>,
    pub edges: Vec<EdgeReport>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderReport {
    /// `RequiredValue`, as `CommonJs`, `Default`, `DefaultWithNamed`,
    /// `Marked`, `Replaced`, or `ExportsObject`.
    pub value: String,
    pub has_default: bool,
    pub has_named_surface: bool,
    pub marks_es_module: bool,
    /// `default`, `namespace`, or `none`.
    pub required_value_import: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdgeReport {
    pub source: String,
    /// False for a call whose span is dummy or shared with another call: it
    /// has no uses and no import tag.
    pub keyed: bool,
    pub form: String,
    pub top_level: bool,
    /// `none`, `default`, or `wildcard`.
    pub interop: String,
    pub esmodule_tested: bool,
    /// The uses on the AST `UnEsm` sees; `None` when that AST does not hold
    /// the call, or the call has no key.
    pub uses: Option<String>,
    /// The import specifiers `UnEsm` made from this call, as `default x`,
    /// `namespace x`, `named a as x`, or `side effect` for a bare import.
    pub un_esm_imports: Vec<String>,
    /// The same after the passes at the fact barrier repaired them.
    pub imports: Vec<String>,
    /// Why no import was made from this call, when none was.
    pub no_import_reason: Option<&'static str>,
}

/// Collects one [`ModuleInterop`] per module from the parallel Phase 2.
#[derive(Default)]
pub(crate) struct InteropSink {
    modules: Mutex<Vec<ModuleInterop>>,
}

impl InteropSink {
    /// Records `module` after the passes at the fact barrier, with the
    /// imports `UnEsm` had made ([`import_forms`] before those passes).
    pub(crate) fn record(
        &self,
        filename: &str,
        facts: Option<&ModuleFacts>,
        plan: &InterfacePlan,
        module: &Module,
        tags: &ImportTags,
        un_esm_imports: ImportForms,
    ) {
        let mut report = module_interop(filename, facts, module, tags, un_esm_imports);
        report.written_by = plan.written_by(filename).map(str::to_string);
        report.helper_exports = plan
            .helper_exports(filename)
            .map(|helpers| {
                helpers
                    .entries()
                    .map(|(name, kind)| {
                        format!(
                            "{} ({})",
                            name.map_or("module.exports", |name| name.as_ref()),
                            match kind {
                                TranspilerHelperKind::InteropRequireWildcard => "wildcard interop",
                                _ => "default interop",
                            }
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.modules
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(report);
    }
}

/// The import specifiers made from each call, by its key, and the sources
/// of side-effect imports.
pub(crate) struct ImportForms {
    by_key: HashMap<EdgeKey, Vec<String>>,
    bare_sources: Vec<String>,
}

impl ImportForms {
    fn of(&self, edge: &RequireEdge) -> Vec<String> {
        let mut forms = self.by_key.get(&edge.key()).cloned().unwrap_or_default();
        if forms.is_empty()
            && matches!(edge.form, RequireForm::Bare)
            && self
                .bare_sources
                .iter()
                .any(|source| *source == *edge.source)
        {
            forms.push("side effect".to_string());
        }
        forms
    }
}

pub(crate) fn import_forms(module: &Module, tags: &ImportTags) -> ImportForms {
    let mut by_key: HashMap<EdgeKey, Vec<String>> = HashMap::default();
    let mut bare_sources = Vec::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        if import.specifiers.is_empty() {
            bare_sources.push(import.src.value.to_string_lossy().into_owned());
        }
        for specifier in &import.specifiers {
            let (local, shown) = match specifier {
                ImportSpecifier::Default(default) => {
                    (&default.local, format!("default {}", default.local.sym))
                }
                ImportSpecifier::Namespace(namespace) => (
                    &namespace.local,
                    format!("namespace {}", namespace.local.sym),
                ),
                ImportSpecifier::Named(named) => {
                    let imported = match &named.imported {
                        Some(ModuleExportName::Ident(ident)) => ident.sym.to_string(),
                        Some(ModuleExportName::Str(name)) => {
                            name.value.to_string_lossy().into_owned()
                        }
                        None => named.local.sym.to_string(),
                    };
                    (
                        &named.local,
                        format!("named {imported} as {}", named.local.sym),
                    )
                }
            };
            for key in tags.edges_of(&binding_id(local)) {
                by_key.entry(*key).or_default().push(shown.clone());
            }
        }
    }
    ImportForms {
        by_key,
        bare_sources,
    }
}

fn module_interop(
    filename: &str,
    facts: Option<&ModuleFacts>,
    module: &Module,
    tags: &ImportTags,
    un_esm_imports: ImportForms,
) -> ModuleInterop {
    let default_facts = ModuleFacts::default();
    let facts = facts.unwrap_or(&default_facts);
    let interface = ProviderInterface::of(facts);
    let provider = ProviderReport {
        value: format!("{:?}", interface.value),
        has_default: interface.has_default,
        has_named_surface: interface.has_named_surface,
        marks_es_module: interface.marks_es_module(),
        required_value_import: match interface.required_value_import() {
            Some(RequiredValueImport::Default) => "default",
            Some(RequiredValueImport::Namespace) => "namespace",
            None => "none",
        },
    };
    let stays_commonjs = !module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)));
    let imports = import_forms(module, tags);

    let edges = facts.require_edges.iter().map(|edge| {
        let un_esm = un_esm_imports.of(edge);
        let imports = imports.of(edge);
        let reason = (un_esm.is_empty() && imports.is_empty()).then(|| {
            if edge.uses.is_none() {
                "the call is not on the AST UnEsm sees"
            } else if stays_commonjs {
                "the module stays CommonJS"
            } else {
                "the require stays a call"
            }
        });
        edge_report(edge, true, un_esm, imports, reason)
    });
    let unkeyed = facts.require_edges.unkeyed().iter().map(|edge| {
        edge_report(
            edge,
            false,
            Vec::new(),
            Vec::new(),
            Some("the call has no key: its span is dummy or shared"),
        )
    });
    ModuleInterop {
        filename: filename.to_string(),
        provider,
        stays_commonjs,
        written_by: None,
        helper_exports: Vec::new(),
        edges: edges.chain(unkeyed).collect(),
    }
}

fn edge_report(
    edge: &RequireEdge,
    keyed: bool,
    un_esm_imports: Vec<String>,
    imports: Vec<String>,
    no_import_reason: Option<&'static str>,
) -> EdgeReport {
    EdgeReport {
        source: edge.source.to_string(),
        keyed,
        form: edge.form.to_string(),
        top_level: edge.top_level,
        interop: edge.interop.to_string(),
        esmodule_tested: edge.esmodule_tested,
        uses: edge.uses.as_ref().map(ToString::to_string),
        un_esm_imports,
        imports,
        no_import_reason,
    }
}

/// Unpacks `source` and reports, per module, its provider interface and its
/// require edges with the import made from each.
pub fn explain_interop(source: &str, mut options: DecompileOptions) -> Result<InteropReport> {
    options.emit_source_map = false;
    let prepared = prepare_unpack_input_with_policy(
        options.filename.clone(),
        source.to_string(),
        true,
        if options.heuristic_split {
            ScopeHoistPolicy::Fallback
        } else {
            ScopeHoistPolicy::Disabled
        },
        false,
    )
    .map_err(super::super::DriverError::into_inner)?;
    let sink = InteropSink::default();
    let nested = nested_scope_split_enabled(&options);
    unpack_prepared_inputs_impl(
        vec![prepared],
        options,
        false,
        if nested {
            ScopeHoistPolicy::Recursive
        } else {
            ScopeHoistPolicy::Fallback
        },
        Some(&sink),
    )?;
    let mut modules = sink
        .modules
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    modules.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(InteropReport { modules })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &str = r#"(() => {
  var __webpack_modules__ = {
    11111: (module, exports, __webpack_require__) => {
      var alpha = __webpack_require__(22222);
      var beta = __webpack_require__(33333);
      beta.gamma = 1;
      exports.delta = alpha.epsilon + beta.zeta;
    },
    22222: (module, exports) => { exports.epsilon = 1; },
    33333: (module, exports) => { exports.zeta = 2; },
  };
  var cache = {};
  function __webpack_require__(id) {
    var cached = cache[id];
    if (cached !== undefined) return cached.exports;
    var module = (cache[id] = { exports: {} });
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__(11111);
})();
"#;

    #[test]
    fn reports_each_edge_with_its_uses_and_the_import_made_from_it() {
        let report = explain_interop(
            BUNDLE,
            DecompileOptions {
                filename: "bundle.js".to_string(),
                ..Default::default()
            },
        )
        .unwrap();
        let consumer = report
            .modules
            .iter()
            .find(|module| module.filename == "module-11111.js")
            .expect("the consumer module");
        let edge = |source: &str| {
            consumer
                .edges
                .iter()
                .find(|edge| edge.source == source)
                .unwrap_or_else(|| panic!("an edge of {source}: {consumer:?}"))
        };

        let alpha = edge("./module-22222.js");
        assert_eq!(alpha.form, "declarator `alpha`");
        assert_eq!(alpha.uses.as_deref(), Some("reads epsilon"));
        assert_eq!(alpha.un_esm_imports, ["default alpha"]);
        assert_eq!(alpha.imports, ["named epsilon as epsilon"]);
        assert_eq!(alpha.no_import_reason, None);

        // A write to the required value keeps the default import.
        let beta = edge("./module-33333.js");
        assert_eq!(beta.uses.as_deref(), Some("reads zeta, writes gamma"));
        assert_eq!(beta.imports, ["default beta"]);

        let provider = report
            .modules
            .iter()
            .find(|module| module.filename == "module-22222.js")
            .unwrap();
        assert_eq!(provider.provider.value, "ExportsObject");
        assert_eq!(provider.provider.required_value_import, "namespace");
    }
}
