//! Dead helper-module elimination for unpack mode.
//!
//! When a transpiler helper (e.g. Babel's `_extends`) lives in its own bundle
//! module and every consumer rewrites away its usage, the consumer's binding
//! import is downgraded by `DeadImports` to a side-effect import
//! `import "./helper.js";`. The helper module then has zero *binding* importers.
//! If it is also a self-contained, side-effect-free helper module, evaluating it
//! does nothing observable, so it is safe to drop — and the now-vacuous
//! side-effect imports in its consumers can be stripped.
//!
//! This is a post-Phase-2 pass: the binding->side-effect downgrade only happens
//! during Phase 2, so the decision needs every module's final import shape. Each
//! module contributes an [`ImportReport`] (computed from its final AST before
//! print); the barrier counts binding importers, picks the drop set, and
//! re-parses only the affected consumers to strip their side-effect imports.

use std::collections::HashSet;

use anyhow::Result;
use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};
use swc_core::ecma::ast::{
    CallExpr, Callee, Decl, Expr, Lit, Module, ModuleDecl, ModuleItem, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::super::io::{parse_js, print_js};
use super::super::types::UnpackWarning;
use super::merge::resolve_relative_specifier;

/// Per-module facts needed to decide dead-module elimination, gathered from the
/// final Phase-2 AST (before print).
pub(super) struct ImportReport {
    /// Static `import ... from "<src>"` declarations as `(source, has_binding)`,
    /// where `has_binding` is true when the import binds specifiers (vs a bare
    /// side-effect import).
    static_imports: Vec<(String, bool)>,
    /// Sources referenced via dynamic `import("<src>")` / `require("<src>")`.
    /// These bind a value, so they keep their target alive.
    dynamic_refs: Vec<String>,
    /// Body contains only declarations + exports and no imports of its own.
    pure_self_contained: bool,
    is_entry: bool,
    /// Phase-1 facts proved this module exports a transpiler helper.
    is_helper: bool,
}

/// Collect the elimination report from a module's final AST.
pub(super) fn collect_import_report(
    module: &Module,
    is_entry: bool,
    is_helper: bool,
) -> ImportReport {
    let mut static_imports = Vec::new();
    for item in &module.body {
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
            if let Some(src) = import.src.value.as_str() {
                static_imports.push((src.to_string(), !import.specifiers.is_empty()));
            }
        }
    }

    let mut dyn_collector = DynamicRefCollector::default();
    module.visit_with(&mut dyn_collector);

    ImportReport {
        static_imports,
        dynamic_refs: dyn_collector.refs,
        pure_self_contained: is_pure_self_contained(module),
        is_entry,
        is_helper,
    }
}

/// Eliminate dead helper modules from the Phase-2 output.
///
/// Drops a module `H` iff it is a recognized helper, its body is pure and
/// self-contained, it is not an entry, and no module imports a binding from it
/// (statically or dynamically). Consumers' now-vacuous side-effect imports of
/// dropped modules are stripped.
pub(super) fn eliminate_dead_helper_modules(
    triples: Vec<(String, String, Vec<UnpackWarning>, Option<ImportReport>)>,
) -> (Vec<(String, String)>, Vec<UnpackWarning>) {
    // Targets reached by a binding (static specifier import or dynamic ref) must
    // be kept. Resolution is in final-name space (Part-1 rename already applied).
    let mut binding_targets: HashSet<String> = HashSet::new();
    for (filename, _, _, report) in &triples {
        let Some(report) = report else { continue };
        for (src, has_binding) in &report.static_imports {
            if *has_binding {
                if let Some(target) = resolve_relative_specifier(filename, src) {
                    binding_targets.insert(target);
                }
            }
        }
        for src in &report.dynamic_refs {
            if let Some(target) = resolve_relative_specifier(filename, src) {
                binding_targets.insert(target);
            }
        }
    }

    let dropped: HashSet<String> = triples
        .iter()
        .filter_map(|(filename, _, _, report)| {
            let report = report.as_ref()?;
            (report.is_helper
                && report.pure_self_contained
                && !report.is_entry
                && !binding_targets.contains(filename))
            .then(|| filename.clone())
        })
        .collect();

    let mut modules = Vec::with_capacity(triples.len());
    let mut warnings = Vec::new();
    for (filename, code, module_warnings, report) in triples {
        if dropped.contains(&filename) {
            continue;
        }
        warnings.extend(module_warnings);

        let imports_dropped = report.as_ref().is_some_and(|report| {
            report.static_imports.iter().any(|(src, has_binding)| {
                !has_binding
                    && resolve_relative_specifier(&filename, src)
                        .is_some_and(|target| dropped.contains(&target))
            })
        });

        if imports_dropped {
            match strip_side_effect_imports(&code, &filename, &dropped) {
                Ok(stripped) => modules.push((filename, stripped)),
                // Re-parse should not fail (we just printed this code), but fall
                // back to the unstripped code rather than losing the module.
                Err(_) => modules.push((filename, code)),
            }
        } else {
            modules.push((filename, code));
        }
    }

    (modules, warnings)
}

/// Re-parse `code` and remove side-effect imports whose source resolves to a
/// dropped module, then reprint.
fn strip_side_effect_imports(
    code: &str,
    from_filename: &str,
    dropped: &HashSet<String>,
) -> Result<String> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(code, from_filename, cm.clone())?;
        module.body.retain(|item| {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                return true;
            };
            if !import.specifiers.is_empty() {
                return true;
            }
            let Some(src) = import.src.value.as_str() else {
                return true;
            };
            !resolve_relative_specifier(from_filename, src)
                .is_some_and(|target| dropped.contains(&target))
        });
        print_js(&module, cm)
    })
}

/// True when every top-level item is a declaration or export and the module has
/// no imports of its own (so evaluating it has no observable side effect and no
/// dependency on another module's evaluation).
fn is_pure_self_contained(module: &Module) -> bool {
    module.body.iter().all(|item| match item {
        ModuleItem::Stmt(Stmt::Decl(decl)) => is_pure_decl(decl),
        ModuleItem::ModuleDecl(decl) => match decl {
            // Any import (static or re-export with a source) makes the module
            // depend on another module's evaluation, so it is not self-contained.
            ModuleDecl::Import(_) | ModuleDecl::ExportAll(_) => false,
            ModuleDecl::ExportNamed(named) => named.src.is_none(),
            ModuleDecl::ExportDecl(export) => is_pure_decl(&export.decl),
            ModuleDecl::ExportDefaultDecl(_) => true,
            ModuleDecl::ExportDefaultExpr(export) => is_pure_init(&export.expr),
            _ => false,
        },
        // Bare expression statements, etc. are potential side effects.
        _ => false,
    })
}

fn is_pure_decl(decl: &Decl) -> bool {
    match decl {
        Decl::Fn(_) | Decl::Class(_) => true,
        Decl::Var(var) => var
            .decls
            .iter()
            .all(|d| d.init.as_ref().is_none_or(|init| is_pure_init(init))),
        Decl::TsInterface(_) | Decl::TsTypeAlias(_) | Decl::TsEnum(_) | Decl::TsModule(_) => true,
        _ => false,
    }
}

fn is_pure_init(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Fn(_) | Expr::Arrow(_) | Expr::Class(_) | Expr::Lit(_) | Expr::Ident(_)
    )
}

#[derive(Default)]
struct DynamicRefCollector {
    refs: Vec<String>,
}

impl DynamicRefCollector {
    fn string_arg(call: &CallExpr) -> Option<String> {
        let arg = call.args.first()?;
        if arg.spread.is_some() {
            return None;
        }
        let Expr::Lit(Lit::Str(s)) = arg.expr.as_ref() else {
            return None;
        };
        s.value.as_str().map(str::to_string)
    }
}

impl Visit for DynamicRefCollector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        let referenced = match &call.callee {
            Callee::Import(_) => Self::string_arg(call),
            Callee::Expr(expr) => match expr.as_ref() {
                Expr::Ident(ident) if ident.sym.as_ref() == "require" => Self::string_arg(call),
                _ => None,
            },
            _ => None,
        };
        if let Some(referenced) = referenced {
            self.refs.push(referenced);
        }
        call.visit_children_with(self);
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::io::parse_js;
    use super::*;
    use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};

    fn parse(source: &str) -> Module {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            parse_js(source, "test.js", cm).expect("source should parse")
        })
    }

    fn report(
        static_imports: Vec<(&str, bool)>,
        dynamic_refs: Vec<&str>,
        pure_self_contained: bool,
        is_entry: bool,
        is_helper: bool,
    ) -> Option<ImportReport> {
        Some(ImportReport {
            static_imports: static_imports
                .into_iter()
                .map(|(s, b)| (s.to_string(), b))
                .collect(),
            dynamic_refs: dynamic_refs.into_iter().map(str::to_string).collect(),
            pure_self_contained,
            is_entry,
            is_helper,
        })
    }

    fn names(modules: &[(String, String)]) -> Vec<&str> {
        modules.iter().map(|(n, _)| n.as_str()).collect()
    }

    #[test]
    fn pure_helper_is_self_contained() {
        assert!(is_pure_self_contained(&parse(
            "function _x() { return 1; } export default _x;"
        )));
    }

    #[test]
    fn module_with_import_is_not_self_contained() {
        assert!(!is_pure_self_contained(&parse(
            "import a from \"./a.js\"; export const x = a;"
        )));
    }

    #[test]
    fn top_level_side_effect_is_not_self_contained() {
        assert!(!is_pure_self_contained(&parse(
            "doThing(); export const x = 1;"
        )));
    }

    #[test]
    fn reexport_from_source_is_not_self_contained() {
        assert!(!is_pure_self_contained(&parse(
            "export { y } from \"./y.js\";"
        )));
    }

    #[test]
    fn report_distinguishes_binding_and_side_effect_imports() {
        let report = collect_import_report(
            &parse("import a from \"./a.js\"; import \"./b.js\";"),
            false,
            true,
        );
        assert!(report
            .static_imports
            .iter()
            .any(|(s, b)| s == "./a.js" && *b));
        assert!(report
            .static_imports
            .iter()
            .any(|(s, b)| s == "./b.js" && !*b));
    }

    #[test]
    fn report_collects_dynamic_refs() {
        let report = collect_import_report(
            &parse("const x = import(\"./a.js\"); const y = require(\"./b.js\");"),
            false,
            false,
        );
        assert!(report.dynamic_refs.contains(&"./a.js".to_string()));
        assert!(report.dynamic_refs.contains(&"./b.js".to_string()));
    }

    #[test]
    fn drops_pure_helper_with_only_side_effect_importer() {
        let triples = vec![
            (
                "helper.js".to_string(),
                "export default function _x() {}".to_string(),
                vec![],
                report(vec![], vec![], true, false, true),
            ),
            (
                "consumer.js".to_string(),
                "import \"./helper.js\";\nexport const x = 1;".to_string(),
                vec![],
                report(vec![("./helper.js", false)], vec![], false, false, false),
            ),
        ];
        let (modules, _) = eliminate_dead_helper_modules(triples);
        assert_eq!(names(&modules), vec!["consumer.js"]);
        let consumer = &modules[0].1;
        assert!(
            !consumer.contains("helper.js"),
            "side-effect import should be stripped:\n{consumer}"
        );
    }

    #[test]
    fn keeps_helper_with_binding_importer() {
        let triples = vec![
            (
                "helper.js".to_string(),
                "export default function _x() {}".to_string(),
                vec![],
                report(vec![], vec![], true, false, true),
            ),
            (
                "consumer.js".to_string(),
                "import _x from \"./helper.js\";".to_string(),
                vec![],
                report(vec![("./helper.js", true)], vec![], false, false, false),
            ),
        ];
        let (modules, _) = eliminate_dead_helper_modules(triples);
        assert!(names(&modules).contains(&"helper.js"));
    }

    #[test]
    fn keeps_non_helper_pure_module() {
        let triples = vec![
            (
                "util.js".to_string(),
                "export default function f() {}".to_string(),
                vec![],
                report(vec![], vec![], true, false, false),
            ),
            (
                "consumer.js".to_string(),
                "import \"./util.js\";".to_string(),
                vec![],
                report(vec![("./util.js", false)], vec![], false, false, false),
            ),
        ];
        let (modules, _) = eliminate_dead_helper_modules(triples);
        assert!(names(&modules).contains(&"util.js"));
    }

    #[test]
    fn keeps_entry_helper_module() {
        let triples = vec![(
            "helper.js".to_string(),
            "export default function _x() {}".to_string(),
            vec![],
            report(vec![], vec![], true, true, true),
        )];
        let (modules, _) = eliminate_dead_helper_modules(triples);
        assert!(names(&modules).contains(&"helper.js"));
    }

    #[test]
    fn keeps_dynamically_referenced_helper() {
        let triples = vec![
            (
                "helper.js".to_string(),
                "export default function _x() {}".to_string(),
                vec![],
                report(vec![], vec![], true, false, true),
            ),
            (
                "consumer.js".to_string(),
                "const m = import(\"./helper.js\");".to_string(),
                vec![],
                report(vec![], vec!["./helper.js"], false, false, false),
            ),
        ];
        let (modules, _) = eliminate_dead_helper_modules(triples);
        assert!(names(&modules).contains(&"helper.js"));
    }
}
