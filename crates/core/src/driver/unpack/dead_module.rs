//! Dead helper-module elimination for unpack mode.
//!
//! When a transpiler helper (e.g. Babel's `_extends`) lives in its own bundle
//! module and every consumer rewrites away its usage, the consumer's binding
//! import is downgraded by `DeadImports` to a side-effect import
//! `import "./helper.js";`. The helper module then has zero *binding* importers.
//! If it is also transitively pure (its own body is side-effect-free and every
//! module it eagerly imports is pure too), evaluating it does nothing
//! observable, so it is safe to drop — and the now-vacuous side-effect imports
//! in its consumers can be stripped. A helper that imports a pure
//! helper-dependency (e.g. `_objectSpread2` -> `_defineProperty`) drops together
//! with its dependency via a cascade.
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
    CallExpr, Callee, Decl, DefaultDecl, Expr, Lit, Module, ModuleDecl, ModuleItem, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::super::io::{parse_js, print_js};
use super::super::types::UnpackWarning;
use crate::module_path::resolve_relative_specifier;

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
    /// The module's own top-level code is side-effect-free (only declarations +
    /// exports). Imports are allowed — whether the imported modules are also pure
    /// is decided transitively at the barrier.
    own_body_pure: bool,
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
        own_body_pure: is_own_body_pure(module),
        is_entry,
        is_helper,
    }
}

/// Eliminate dead helper modules from the Phase-2 output.
///
/// Drops a module `H` iff it is a recognized helper, transitively pure (its own
/// body is side-effect-free and every module it eagerly imports is also pure),
/// not an entry, and every module that binding-imports it is itself dropped
/// (cascade). This lets a helper that imports a pure helper-dependency (e.g.
/// `_objectSpread2` -> `_defineProperty`) be dropped together with its
/// dependency. Consumers' now-vacuous side-effect imports of dropped modules are
/// stripped.
pub(super) fn eliminate_dead_helper_modules(
    triples: Vec<(String, String, Vec<UnpackWarning>, Option<ImportReport>)>,
) -> (Vec<(String, String)>, Vec<UnpackWarning>) {
    // A module that failed to decompile has no report, so we cannot see its
    // references and cannot prove another module is unused. Bail conservatively.
    if triples.iter().any(|(_, _, _, report)| report.is_none()) {
        let mut modules = Vec::with_capacity(triples.len());
        let mut warnings = Vec::new();
        for (filename, code, module_warnings, _) in triples {
            modules.push((filename, code));
            warnings.extend(module_warnings);
        }
        return (modules, warnings);
    }

    let dropped = compute_dropped(&triples);

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

/// Compute the set of module filenames to drop, via two fixpoints over the
/// module graph (all reports are present — callers bail on missing ones).
fn compute_dropped(
    triples: &[(String, String, Vec<UnpackWarning>, Option<ImportReport>)],
) -> HashSet<String> {
    let report_of = |filename: &str| -> &ImportReport {
        triples
            .iter()
            .find(|(name, ..)| name == filename)
            .and_then(|(_, _, _, report)| report.as_ref())
            .expect("reports are present")
    };
    let all: HashSet<&str> = triples.iter().map(|(name, ..)| name.as_str()).collect();

    // Per-module: eager in-set import deps, an external-dep flag (any import that
    // is bare or resolves outside the bundle), and the binding-importer edges.
    let mut in_deps: std::collections::HashMap<&str, Vec<String>> =
        std::collections::HashMap::new();
    let mut has_external: HashSet<&str> = HashSet::new();
    let mut binding_importers: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for (filename, _, _, report) in triples {
        let report = report.as_ref().expect("reports are present");
        let mut deps = Vec::new();
        for (src, has_binding) in &report.static_imports {
            match resolve_relative_specifier(filename, src) {
                Some(target) if all.contains(target.as_str()) => {
                    if *has_binding {
                        binding_importers
                            .entry(target.clone())
                            .or_default()
                            .push(filename.clone());
                    }
                    deps.push(target);
                }
                // Bare or out-of-bundle import: an eager dependency we cannot
                // prove pure, so the module is not transitively pure.
                _ => {
                    has_external.insert(filename.as_str());
                }
            }
        }
        for src in &report.dynamic_refs {
            if let Some(target) = resolve_relative_specifier(filename, src) {
                if all.contains(target.as_str()) {
                    binding_importers
                        .entry(target)
                        .or_default()
                        .push(filename.clone());
                }
            }
        }
        in_deps.insert(filename.as_str(), deps);
    }

    // Fixpoint 1 — transitive purity: a module is pure iff its own body is pure,
    // it has no external eager dep, and every in-bundle eager dep is also pure.
    let mut pure: HashSet<String> = triples
        .iter()
        .filter(|(name, ..)| report_of(name).own_body_pure && !has_external.contains(name.as_str()))
        .map(|(name, ..)| name.clone())
        .collect();
    loop {
        let remove: Vec<String> = pure
            .iter()
            .filter(|name| in_deps[name.as_str()].iter().any(|dep| !pure.contains(dep)))
            .cloned()
            .collect();
        if remove.is_empty() {
            break;
        }
        for name in remove {
            pure.remove(&name);
        }
    }

    // Fixpoint 2 — cascade: start from droppable candidates (recognized helper,
    // transitively pure, non-entry), then remove any whose binding-importer is
    // still live (not itself dropped). Dropping a helper removes its own binding
    // imports, so a helper-dependency whose only binding importer is dropped
    // becomes droppable too.
    let mut candidate: HashSet<String> = triples
        .iter()
        .filter(|(name, ..)| {
            let report = report_of(name);
            report.is_helper && !report.is_entry && pure.contains(name.as_str())
        })
        .map(|(name, ..)| name.clone())
        .collect();
    loop {
        let remove: Vec<String> = candidate
            .iter()
            .filter(|name| {
                binding_importers
                    .get(name.as_str())
                    .is_some_and(|importers| importers.iter().any(|m| !candidate.contains(m)))
            })
            .cloned()
            .collect();
        if remove.is_empty() {
            break;
        }
        for name in remove {
            candidate.remove(&name);
        }
    }

    candidate
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

/// True when the module's own top-level code has no side effects: every item is
/// a declaration, a plain import, or a local export. Imports are allowed (an
/// import declaration runs no code here); whether the imported modules are
/// themselves pure is decided transitively at the barrier. Re-exports with a
/// source (`export ... from "x"`, `export *`) are treated conservatively as
/// disqualifying, so the only eager dependencies to reason about are imports.
fn is_own_body_pure(module: &Module) -> bool {
    module.body.iter().all(|item| match item {
        ModuleItem::Stmt(Stmt::Decl(decl)) => is_pure_decl(decl),
        ModuleItem::ModuleDecl(decl) => match decl {
            ModuleDecl::Import(_) => true,
            ModuleDecl::ExportAll(_) => false,
            ModuleDecl::ExportNamed(named) => named.src.is_none(),
            ModuleDecl::ExportDecl(export) => is_pure_decl(&export.decl),
            ModuleDecl::ExportDefaultDecl(export) => is_pure_default_decl(&export.decl),
            ModuleDecl::ExportDefaultExpr(export) => is_pure_init(&export.expr),
            _ => false,
        },
        // Bare expression statements, etc. are potential side effects.
        _ => false,
    })
}

fn is_pure_decl(decl: &Decl) -> bool {
    match decl {
        Decl::Fn(_) => true,
        Decl::Class(_) => false,
        Decl::Var(var) => var
            .decls
            .iter()
            .all(|d| d.init.as_ref().is_none_or(|init| is_pure_init(init))),
        Decl::TsInterface(_) | Decl::TsTypeAlias(_) | Decl::TsEnum(_) | Decl::TsModule(_) => true,
        _ => false,
    }
}

fn is_pure_default_decl(decl: &DefaultDecl) -> bool {
    matches!(decl, DefaultDecl::Fn(_) | DefaultDecl::TsInterfaceDecl(_))
}

fn is_pure_init(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Fn(_) | Expr::Arrow(_) | Expr::Lit(_) | Expr::Ident(_)
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
        own_body_pure: bool,
        is_entry: bool,
        is_helper: bool,
    ) -> Option<ImportReport> {
        Some(ImportReport {
            static_imports: static_imports
                .into_iter()
                .map(|(s, b)| (s.to_string(), b))
                .collect(),
            dynamic_refs: dynamic_refs.into_iter().map(str::to_string).collect(),
            own_body_pure,
            is_entry,
            is_helper,
        })
    }

    fn names(modules: &[(String, String)]) -> Vec<&str> {
        modules.iter().map(|(n, _)| n.as_str()).collect()
    }

    #[test]
    fn pure_helper_body_is_pure() {
        assert!(is_own_body_pure(&parse(
            "function _x() { return 1; } export default _x;"
        )));
    }

    #[test]
    fn import_alone_does_not_make_body_impure() {
        // own-body purity allows imports; whether the dep is pure is transitive.
        assert!(is_own_body_pure(&parse(
            "import a from \"./a.js\"; export const x = a;"
        )));
    }

    #[test]
    fn top_level_side_effect_is_not_pure() {
        assert!(!is_own_body_pure(&parse("doThing(); export const x = 1;")));
    }

    #[test]
    fn class_declaration_is_not_pure() {
        assert!(!is_own_body_pure(&parse(
            "class C { static { sideEffect(); } } export default function _x() {}"
        )));
    }

    #[test]
    fn export_default_class_is_not_pure() {
        assert!(!is_own_body_pure(&parse(
            "export default class C { static { sideEffect(); } }"
        )));
    }

    #[test]
    fn reexport_from_source_is_not_pure() {
        assert!(!is_own_body_pure(&parse("export { y } from \"./y.js\";")));
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

    #[test]
    fn drops_helper_chain_via_transitive_purity_and_cascade() {
        // objectSpread2 binding-imports defineProperty; the consumer only keeps a
        // side-effect import of objectSpread2. Both helpers are pure, so both drop.
        let triples = vec![
            (
                "defineProperty.js".to_string(),
                "export default function _dp() {}".to_string(),
                vec![],
                report(vec![], vec![], true, false, true),
            ),
            (
                "objectSpread2.js".to_string(),
                "import dp from \"./defineProperty.js\";\nexport default function _os() { return dp; }".to_string(),
                vec![],
                report(vec![("./defineProperty.js", true)], vec![], true, false, true),
            ),
            (
                "consumer.js".to_string(),
                "import \"./objectSpread2.js\";\nexport const x = 1;".to_string(),
                vec![],
                report(vec![("./objectSpread2.js", false)], vec![], false, false, false),
            ),
        ];
        let (modules, _) = eliminate_dead_helper_modules(triples);
        assert_eq!(names(&modules), vec!["consumer.js"]);
    }

    #[test]
    fn keeps_helper_with_external_dependency() {
        // helper imports an out-of-bundle module, so it is not transitively pure.
        let triples = vec![
            (
                "helper.js".to_string(),
                "import x from \"react\";\nexport default function _h() { return x; }".to_string(),
                vec![],
                report(vec![("react", true)], vec![], true, false, true),
            ),
            (
                "consumer.js".to_string(),
                "import \"./helper.js\";".to_string(),
                vec![],
                report(vec![("./helper.js", false)], vec![], false, false, false),
            ),
        ];
        let (modules, _) = eliminate_dead_helper_modules(triples);
        assert!(names(&modules).contains(&"helper.js"));
    }
}
