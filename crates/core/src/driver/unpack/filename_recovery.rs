//! Recover original module filenames from `@sentry/babel-plugin-component-annotate`
//! provenance markers.
//!
//! The Sentry Babel plugin annotates JSX at the original source level with
//! `data-sentry-source-file="<file>"` (and `data-sentry-component`). After the
//! application's own JSX transform + minification, that annotation survives in
//! the bundle as a string-literal property in the `jsx`/`createElement` props
//! object — e.g. `_jsx("div", { "data-sentry-component": "Foo",
//! "data-sentry-source-file": "Foo.jsx", ... })`.
//!
//! Because `data-sentry-source-file` names the file the JSX is *written in*,
//! every annotated element in one source module carries the same value. We
//! harvest it in Phase 1 (before `UnJsx` runs, so it is still an object
//! property), build a `provisional -> recovered` filename table at the
//! cross-module barrier, then rename the module's output file and rewrite
//! importers' import-source strings to match.
//!
//! The rename is applied as a final, isolated remap: the cross-module fact
//! system, numeric rewrites, and namespace decomposition all keep operating on
//! provisional filenames; only the last step before emit swaps names.

use std::collections::{HashMap, HashSet};

use swc_core::common::Mark;
use swc_core::ecma::ast::{
    CallExpr, Callee, ExportAll, Expr, ImportDecl, KeyValueProp, Lit, Module, NamedExport,
    ObjectLit, Prop, PropName, PropOrSpread, Str,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::super::output::safe_relative_module_path;
use crate::module_path::{relative_import_specifier, resolve_relative_specifier};

const SOURCE_FILE_KEYS: &[&str] = &["data-sentry-source-file", "dataSentrySourceFile"];
const COMPONENT_KEYS: &[&str] = &["data-sentry-component", "dataSentryComponent"];

/// Harvest the original source filename advertised by Sentry annotations in a
/// freshly-parsed module, if any. Only accepts values co-located with a
/// `data-sentry-component` marker, matching the Sentry plugin shape and avoiding
/// unrelated objects that happen to carry a `data-sentry-source-file` key.
/// Returns `None` when the module has no complete marker or carries conflicting
/// filenames (e.g. several concatenated source files), so naming stays
/// conservative.
pub(super) fn harvest_suggested_filename(module: &Module) -> Option<String> {
    let mut collector = SentrySourceFileCollector::default();
    module.visit_with(&mut collector);
    collector.choose()
}

#[derive(Default)]
struct SentrySourceFileCollector {
    counts: HashMap<String, usize>,
}

impl SentrySourceFileCollector {
    fn choose(&self) -> Option<String> {
        if self.counts.len() == 1 {
            return self.counts.keys().next().cloned();
        }
        None
    }
}

impl Visit for SentrySourceFileCollector {
    fn visit_object_lit(&mut self, obj: &ObjectLit) {
        let mut source_file = None;
        let mut has_component = false;
        for prop in &obj.props {
            let PropOrSpread::Prop(prop) = prop else {
                continue;
            };
            let Prop::KeyValue(KeyValueProp { key, value }) = prop.as_ref() else {
                continue;
            };
            let Some(name) = prop_key_name(key) else {
                continue;
            };
            if SOURCE_FILE_KEYS.contains(&name) {
                if let Expr::Lit(Lit::Str(s)) = value.as_ref() {
                    if let Some(value) = s.value.as_str() {
                        if !value.is_empty() {
                            source_file = Some(value.to_string());
                        }
                    }
                }
            } else if COMPONENT_KEYS.contains(&name) {
                has_component = true;
            }
        }
        if has_component {
            if let Some(source_file) = source_file {
                *self.counts.entry(source_file).or_default() += 1;
            }
        }
        obj.visit_children_with(self);
    }
}

fn prop_key_name(key: &PropName) -> Option<&str> {
    match key {
        PropName::Str(s) => s.value.as_str(),
        PropName::Ident(i) => Some(i.sym.as_ref()),
        _ => None,
    }
}

/// Build the `provisional_filename -> recovered_filename` rename table from each
/// module's `(filename, suggested)` pair.
///
/// Conservative by construction:
/// - skips unsafe recovered paths (absolute, `..`-escaping);
/// - skips a recovered name claimed by more than one module (ambiguous);
/// - skips a recovered name that collides with another module's existing
///   provisional filename (would clobber a reference target).
pub(super) fn build_rename_map(entries: &[(String, Option<String>)]) -> HashMap<String, String> {
    let provisional: HashSet<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
    let mut target_count: HashMap<String, usize> = HashMap::new();
    let mut candidates: Vec<(String, String)> = Vec::new();
    for (provisional_name, suggested) in entries {
        let Some(suggested) = suggested else {
            continue;
        };
        let Ok(safe) = safe_relative_module_path(suggested) else {
            continue;
        };
        let recovered = safe.to_string_lossy().replace('\\', "/");
        if &recovered == provisional_name {
            continue;
        }
        *target_count.entry(recovered.clone()).or_default() += 1;
        candidates.push((provisional_name.clone(), recovered));
    }

    let mut map = HashMap::new();
    for (provisional_name, recovered) in candidates {
        if target_count.get(&recovered).copied().unwrap_or(0) > 1 {
            continue;
        }
        if provisional.contains(recovered.as_str()) {
            continue;
        }
        map.insert(provisional_name, recovered);
    }
    map
}

/// Rewrite import/export-from/dynamic-import source strings in `module` so that
/// references to renamed modules point at their recovered filenames. `module`
/// belongs to `from_filename` (its provisional name); references are resolved
/// relative to that and re-formed relative to the importer's final name.
pub(super) fn rewrite_import_sources(
    module: &mut Module,
    from_filename: &str,
    rename_map: &HashMap<String, String>,
    unresolved_mark: Mark,
) {
    let mut rewriter = ImportSourceRewriter {
        from_filename,
        rename_map,
        unresolved_mark,
    };
    module.visit_mut_with(&mut rewriter);
}

struct ImportSourceRewriter<'a> {
    from_filename: &'a str,
    rename_map: &'a HashMap<String, String>,
    unresolved_mark: Mark,
}

impl ImportSourceRewriter<'_> {
    fn rewrite(&self, src: &mut Str) {
        let Some(spec) = src.value.as_str() else {
            return;
        };
        let Some(target) = resolve_relative_specifier(self.from_filename, spec) else {
            return;
        };
        let Some(recovered) = self.rename_map.get(&target) else {
            return;
        };
        let from_final = self
            .rename_map
            .get(self.from_filename)
            .map(String::as_str)
            .unwrap_or(self.from_filename);
        let new_spec = relative_import_specifier(from_final, recovered);
        src.value = new_spec.into();
        src.raw = None;
    }
}

impl VisitMut for ImportSourceRewriter<'_> {
    fn visit_mut_import_decl(&mut self, n: &mut ImportDecl) {
        self.rewrite(&mut n.src);
        n.visit_mut_children_with(self);
    }

    fn visit_mut_named_export(&mut self, n: &mut NamedExport) {
        if let Some(src) = &mut n.src {
            self.rewrite(src);
        }
        n.visit_mut_children_with(self);
    }

    fn visit_mut_export_all(&mut self, n: &mut ExportAll) {
        self.rewrite(&mut n.src);
        n.visit_mut_children_with(self);
    }

    fn visit_mut_call_expr(&mut self, n: &mut CallExpr) {
        let should_rewrite = match &n.callee {
            Callee::Import(_) => true,
            Callee::Expr(callee) => matches!(
                callee.as_ref(),
                Expr::Ident(ident)
                    if ident.sym.as_ref() == "require"
                        && ident.ctxt.outer() == self.unresolved_mark
            ),
            _ => false,
        };
        if should_rewrite {
            if let Some(arg) = n.args.first_mut() {
                if arg.spread.is_none() {
                    if let Expr::Lit(Lit::Str(s)) = arg.expr.as_mut() {
                        self.rewrite(s);
                    }
                }
            }
        }
        n.visit_mut_children_with(self);
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::io::{parse_js, print_js};
    use super::*;
    use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
    use swc_core::ecma::transforms::base::resolver;

    fn harvest(source: &str) -> Option<String> {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let module = parse_js(source, "test.js", cm).expect("source should parse");
            harvest_suggested_filename(&module)
        })
    }

    #[test]
    fn harvests_source_file_from_props_object() {
        let got = harvest(
            r#"_jsx("div", {
                "data-sentry-component": "Widget",
                "data-sentry-source-file": "Widget.jsx",
                children: "x"
            });"#,
        );
        assert_eq!(got.as_deref(), Some("Widget.jsx"));
    }

    #[test]
    fn harvests_native_camelcase_attribute() {
        let got = harvest(
            r#"_jsx("div", {
                dataSentryComponent: "Widget",
                dataSentrySourceFile: "Widget.jsx"
            });"#,
        );
        assert_eq!(got.as_deref(), Some("Widget.jsx"));
    }

    #[test]
    fn prefers_component_colocated_source_file() {
        // The root carries the component marker; a sibling object carries a
        // different source-file. The component-co-located value wins.
        let got = harvest(
            r#"
            _jsx("div", {
                "data-sentry-component": "Root",
                "data-sentry-source-file": "Root.jsx"
            });
            _jsx("span", { "data-sentry-source-file": "Other.jsx" });
            "#,
        );
        assert_eq!(got.as_deref(), Some("Root.jsx"));
    }

    #[test]
    fn backs_off_on_conflicting_source_files_without_component() {
        let got = harvest(
            r#"
            _jsx("a", { "data-sentry-source-file": "A.jsx" });
            _jsx("b", { "data-sentry-source-file": "B.jsx" });
            "#,
        );
        assert_eq!(got, None, "ambiguous source files should not pick a name");
    }

    #[test]
    fn ignores_source_file_without_component_marker() {
        let got = harvest(r#"const meta = { "data-sentry-source-file": "Widget.jsx" };"#);
        assert_eq!(
            got, None,
            "source-file alone is too broad to prove a module filename"
        );
    }

    #[test]
    fn no_marker_yields_none() {
        assert_eq!(harvest(r#"_jsx("div", { className: "x" });"#), None);
    }

    #[test]
    fn rename_map_renames_unique_recovered_name() {
        let entries = vec![
            ("a.js".to_string(), Some("Widget.jsx".to_string())),
            ("b.js".to_string(), None),
        ];
        let map = build_rename_map(&entries);
        assert_eq!(map.get("a.js").map(String::as_str), Some("Widget.jsx"));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn rename_map_drops_recovered_name_claimed_by_multiple_modules() {
        let entries = vec![
            ("a.js".to_string(), Some("Shared.jsx".to_string())),
            ("b.js".to_string(), Some("Shared.jsx".to_string())),
        ];
        assert!(
            build_rename_map(&entries).is_empty(),
            "ambiguous recovered names should be dropped to keep references unambiguous"
        );
    }

    #[test]
    fn rename_map_drops_collision_with_existing_provisional_name() {
        // Recovering "b.js" for module a.js would clobber the real module b.js.
        let entries = vec![
            ("a.js".to_string(), Some("b.js".to_string())),
            ("b.js".to_string(), None),
        ];
        assert!(build_rename_map(&entries).is_empty());
    }

    #[test]
    fn rename_map_skips_unsafe_recovered_paths() {
        let entries = vec![("a.js".to_string(), Some("../escape.js".to_string()))];
        assert!(build_rename_map(&entries).is_empty());
    }

    #[test]
    fn rename_map_skips_noop_recovery() {
        let entries = vec![("a.js".to_string(), Some("a.js".to_string()))];
        assert!(build_rename_map(&entries).is_empty());
    }

    #[test]
    fn resolves_relative_specifiers_to_module_keys() {
        assert_eq!(
            resolve_relative_specifier("b.js", "./a.js").as_deref(),
            Some("a.js")
        );
        assert_eq!(
            resolve_relative_specifier("src/b.js", "./a.js").as_deref(),
            Some("src/a.js")
        );
        assert_eq!(
            resolve_relative_specifier("src/b.js", "../a.js").as_deref(),
            Some("a.js")
        );
        assert_eq!(resolve_relative_specifier("b.js", "react"), None);
    }

    fn rewrite(source: &str) -> String {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module =
                parse_js(source, "consumer.js", cm.clone()).expect("source should parse");
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
            let rename_map = HashMap::from([("a.js".to_string(), "Widget.jsx".to_string())]);
            rewrite_import_sources(&mut module, "consumer.js", &rename_map, unresolved_mark);
            print_js(&module, cm).expect("module should print")
        })
    }

    #[test]
    fn rewrites_unresolved_require_source() {
        let got = rewrite(r#"export default require("./a.js");"#);
        assert!(
            got.contains(r#"require("./Widget.jsx")"#),
            "unresolved require should point at recovered filename:\n{got}"
        );
    }

    #[test]
    fn does_not_rewrite_shadowed_require_source() {
        let got = rewrite(
            r#"
function require(x) {
    return x;
}
export default require("./a.js");
"#,
        );
        assert!(
            got.contains(r#"require("./a.js")"#),
            "local require binding should not be treated as CommonJS:\n{got}"
        );
    }
}
