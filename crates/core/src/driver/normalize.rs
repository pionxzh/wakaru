//! Source canonicalization used for structure-only comparison.
//!
//! [`normalize`] parses a snippet, runs the resolver so every identifier carries
//! a [`SyntaxContext`], and (optionally) renames every *local* binding to a
//! deterministic, position-based name (`$0`, `$1`, …) while leaving free/global
//! identifiers untouched. Two programs that differ only by binding names — for
//! example an original snippet and its minifier-mangled form — normalize to
//! identical source, which lets tests and the reproduction matrices assert
//! structural equality without being sensitive to mangling or formatting.
//!
//! Unlike a regex-based name collapse, this is scope-correct (it reuses the
//! sanctioned [`rename_bindings_in_module`]) and *non-lossy*: distinct bindings
//! keep distinct canonical names, so `load_backup(x)` and `load_meta(x)` never
//! collapse to the same shape.

use anyhow::Result;
use std::collections::HashSet;

use swc_core::common::{sync::Lrc, Mark, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{Ident, Module};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use super::io::{parse_js_with_recovery, print_js};
use crate::rules::rename_utils::{rename_bindings_in_module, BindingId, BindingRename};

/// Options controlling [`normalize`].
#[derive(Debug, Clone, Default)]
pub struct NormalizeOptions {
    /// Rename every local binding to a deterministic, position-based canonical
    /// name (`$0`, `$1`, …). Free/global identifiers are left untouched, so two
    /// alpha-equivalent programs normalize to identical source.
    pub rename_bindings: bool,
    /// Filename hint used for syntax detection (`.ts`/`.tsx`/`.jsx`). Defaults to
    /// `input.js` when empty.
    pub filename: String,
}

impl NormalizeOptions {
    /// Canonicalize formatting only (reprint through the parser/printer).
    pub fn format_only() -> Self {
        Self::default()
    }

    /// Canonicalize formatting *and* alpha-rename local bindings.
    pub fn with_rename() -> Self {
        Self {
            rename_bindings: true,
            ..Self::default()
        }
    }
}

/// Parse `source`, resolve scopes, optionally alpha-rename local bindings, and
/// reprint. The result is a canonical form suitable for structural comparison.
pub fn normalize(source: &str, options: &NormalizeOptions) -> Result<String> {
    let filename = if options.filename.is_empty() {
        "input.js"
    } else {
        options.filename.as_str()
    };

    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js_with_recovery(source, filename, cm.clone())?.module;

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        if options.rename_bindings {
            let renames = canonical_binding_renames(&module, unresolved_mark);
            rename_bindings_in_module(&mut module, &renames);
        }

        print_js(&module, cm)
    })
}

/// Build a position-based rename for every local binding, in first-encounter
/// (pre-order) traversal order. Because the traversal order is structural, two
/// alpha-equivalent modules produce identical renames and thus identical output.
fn canonical_binding_renames(module: &Module, unresolved_mark: Mark) -> Vec<BindingRename> {
    let mut collector = BindingOrderCollector {
        unresolved_mark,
        seen: HashSet::new(),
        order: Vec::new(),
    };
    module.visit_with(&mut collector);
    collector
        .order
        .into_iter()
        .enumerate()
        .map(|(index, old)| BindingRename {
            old,
            new: format!("${index}").into(),
        })
        .collect()
}

struct BindingOrderCollector {
    unresolved_mark: Mark,
    seen: HashSet<BindingId>,
    order: Vec<BindingId>,
}

impl BindingOrderCollector {
    fn record(&mut self, ident: &Ident) {
        let ctxt = ident.ctxt;
        // Skip free variables: globals (`Object`, `require`, undeclared names)
        // carry the unresolved mark, and the empty context means resolver never
        // bound the identifier. Only resolver-bound locals are renamed. This is
        // the same `unresolved_mark` gate every scope-aware rule uses.
        if ctxt == SyntaxContext::empty() || ctxt.outer() == self.unresolved_mark {
            return;
        }
        let key = (ident.sym.clone(), ctxt);
        if self.seen.insert(key.clone()) {
            self.order.push(key);
        }
    }
}

impl Visit for BindingOrderCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.record(ident);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renamed(source: &str) -> String {
        normalize(source, &NormalizeOptions::with_rename()).expect("normalize")
    }

    #[test]
    fn alpha_equivalent_programs_normalize_identically() {
        let original = "function load(app_id) { return fetch_user(app_id); }";
        let mangled = "function l(e) { return fetch_user(e); }";
        assert_eq!(renamed(original), renamed(mangled));
    }

    #[test]
    fn global_references_are_preserved() {
        // `fetch_user` is free; it must survive renaming so it can anchor the
        // comparison.
        let out = renamed("function f(x) { return fetch_user(x); }");
        assert!(out.contains("fetch_user"), "globals preserved: {out}");
        assert!(!out.contains("function f"), "local renamed: {out}");
    }

    #[test]
    fn distinct_bindings_stay_distinct() {
        // Regression for the lossy `_`-collapse: distinct call targets and
        // distinct locals must not normalize to the same shape.
        let keep_meta = "function f(id) { return load_meta(id); }";
        let keep_backup = "function f(id) { return load_backup(id); }";
        assert_ne!(renamed(keep_meta), renamed(keep_backup));
    }

    #[test]
    fn property_keys_are_not_renamed() {
        // Object keys / member props are not bindings and must be preserved,
        // even when a local shares the name.
        let out = renamed("function f(key) { const o = { key: key }; return o.key; }");
        assert!(
            out.contains("o.key") || out.contains(".key"),
            "member prop kept: {out}"
        );
    }

    #[test]
    fn nested_same_name_locals_get_distinct_canonical_names() {
        // Two `x` in sibling scopes are different bindings; alpha-equivalence
        // must hold against a version that uses different names per scope.
        let a = "function f() { { let x = 1; use(x); } { let x = 2; use(x); } }";
        let b = "function f() { { let p = 1; use(p); } { let q = 2; use(q); } }";
        assert_eq!(renamed(a), renamed(b));
    }

    #[test]
    fn format_only_does_not_rename() {
        let out = normalize(
            "function load(app_id){return app_id}",
            &NormalizeOptions::format_only(),
        )
        .expect("normalize");
        assert!(
            out.contains("load"),
            "binding preserved without rename: {out}"
        );
        assert!(
            out.contains("app_id"),
            "param preserved without rename: {out}"
        );
    }
}
