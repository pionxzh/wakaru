use std::collections::HashMap;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::{
    BindingIdent, ClassDecl, ForInStmt, ForOfStmt, ForStmt, ImportDecl, ImportSpecifier,
    ModuleItem, ObjectPatProp, Pat, VarDecl, VarDeclKind,
};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use super::io::{parse_js_with_recovery, parse_script_with_recovery, ParseDiagnostic};
use super::types::{UnpackWarning, UnpackWarningKind};

pub(super) fn collect_tdz_warnings(
    module: &swc_core::ecma::ast::Module,
    filename: &str,
) -> Vec<UnpackWarning> {
    crate::tdz_check::check_tdz(module)
        .into_iter()
        .map(|v| {
            UnpackWarning::new(
                filename,
                UnpackWarningKind::TdzViolation,
                format!("reference to `{}` before declaration", v.name),
            )
        })
        .collect()
}

pub(super) fn collect_input_parse_warnings(errors: &[ParseDiagnostic]) -> Vec<UnpackWarning> {
    // Source locations identify occurrences, not distinct parser conditions.
    // Keep the first-seen signature order while collapsing repeated conditions
    // within one parsed file.
    let mut group_indexes: HashMap<(&str, &str), usize> = HashMap::new();
    let mut groups: Vec<(&ParseDiagnostic, usize)> = Vec::new();

    for error in errors {
        let signature = (error.filename.as_str(), error.message.as_str());
        if let Some(index) = group_indexes.get(&signature).copied() {
            groups[index].1 += 1;
        } else {
            group_indexes.insert(signature, groups.len());
            groups.push((error, 1));
        }
    }

    groups
        .into_iter()
        .map(|(first, occurrences)| {
            let message = if occurrences == 1 {
                format!("input parse recovered from parser error: {first}")
            } else {
                format!(
                    "input parse recovered from repeated parser error {} ({occurrences} occurrences; first at {}:{}:{})",
                    first.message, first.filename, first.line, first.column
                )
            };
            UnpackWarning::new(
                &first.filename,
                UnpackWarningKind::InputParseRecovered,
                message,
            )
        })
        .collect()
}

pub(super) fn collect_duplicate_declaration_warnings(
    module: &swc_core::ecma::ast::Module,
    filename: &str,
) -> Vec<UnpackWarning> {
    let mut collector = DuplicateDeclarationCollector::default();
    module.visit_with(&mut collector);
    collector
        .duplicates
        .into_iter()
        .map(|name| {
            UnpackWarning::new(
                filename,
                UnpackWarningKind::DuplicateDeclaration,
                format!("duplicate lexical declaration `{name}`"),
            )
        })
        .collect()
}

#[derive(Default)]
struct DuplicateDeclarationCollector {
    seen: HashMap<(Atom, swc_core::common::SyntaxContext), ()>,
    duplicates: Vec<Atom>,
}

impl DuplicateDeclarationCollector {
    fn record_binding(&mut self, binding: &BindingIdent) {
        let key = (binding.id.sym.clone(), binding.id.ctxt);
        if self.seen.insert(key, ()).is_some() && !self.duplicates.contains(&binding.id.sym) {
            self.duplicates.push(binding.id.sym.clone());
        }
    }

    fn record_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(binding) => self.record_binding(binding),
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.record_pat(elem);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::KeyValue(kv) => self.record_pat(&kv.value),
                        ObjectPatProp::Assign(assign) => {
                            self.record_binding(&assign.key);
                        }
                        ObjectPatProp::Rest(rest) => self.record_pat(&rest.arg),
                    }
                }
            }
            Pat::Rest(rest) => self.record_pat(&rest.arg),
            Pat::Assign(assign) => self.record_pat(&assign.left),
            Pat::Expr(_) | Pat::Invalid(_) => {}
        }
    }
}

impl Visit for DuplicateDeclarationCollector {
    fn visit_class_decl(&mut self, class_decl: &ClassDecl) {
        self.record_binding(&BindingIdent {
            id: class_decl.ident.clone(),
            type_ann: None,
        });
        class_decl.class.visit_with(self);
    }

    fn visit_import_decl(&mut self, import_decl: &ImportDecl) {
        for specifier in &import_decl.specifiers {
            match specifier {
                ImportSpecifier::Named(named) => self.record_binding(&BindingIdent {
                    id: named.local.clone(),
                    type_ann: None,
                }),
                ImportSpecifier::Default(default) => self.record_binding(&BindingIdent {
                    id: default.local.clone(),
                    type_ann: None,
                }),
                ImportSpecifier::Namespace(namespace) => self.record_binding(&BindingIdent {
                    id: namespace.local.clone(),
                    type_ann: None,
                }),
            }
        }
    }

    fn visit_var_decl(&mut self, var_decl: &VarDecl) {
        if var_decl.kind == VarDeclKind::Var {
            return;
        }
        for decl in &var_decl.decls {
            self.record_pat(&decl.name);
        }
        var_decl.visit_children_with(self);
    }

    fn visit_block_stmt(&mut self, block: &swc_core::ecma::ast::BlockStmt) {
        let mut child = DuplicateDeclarationCollector::default();
        block.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_function(&mut self, func: &swc_core::ecma::ast::Function) {
        let mut child = DuplicateDeclarationCollector::default();
        func.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        let mut child = DuplicateDeclarationCollector::default();
        arrow.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_class(&mut self, class: &swc_core::ecma::ast::Class) {
        let mut child = DuplicateDeclarationCollector::default();
        class.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        let mut child = DuplicateDeclarationCollector::default();
        stmt.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        let mut child = DuplicateDeclarationCollector::default();
        stmt.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        let mut child = DuplicateDeclarationCollector::default();
        stmt.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }
}

/// Validate the text users receive, then resolve that emitted program from
/// scratch before running identity-sensitive diagnostics. Transform rules can
/// legally change lexical scope without rebuilding every pre-transform
/// `SyntaxContext`; those internal contexts must not become user warnings.
pub(super) fn collect_output_diagnostics(code: &str, filename: &str) -> Vec<UnpackWarning> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        match parse_js_with_recovery(code, filename, cm) {
            Ok(parsed) if parsed.recoverable_errors.is_empty() => {
                collect_resolved_output_warnings(parsed.module, filename)
            }
            Ok(parsed)
                if parsed
                    .module
                    .body
                    .iter()
                    .any(|item| matches!(item, ModuleItem::ModuleDecl(_))) =>
            {
                output_parse_warnings(parsed.recoverable_errors, filename)
            }
            Ok(parsed) => match parse_script_with_recovery(code, filename, Default::default()) {
                Ok(script) if script.recoverable_errors.is_empty() => {
                    collect_resolved_output_warnings(script.module, filename)
                }
                Ok(script) => output_parse_warnings(script.recoverable_errors, filename),
                Err(_) => output_parse_warnings(parsed.recoverable_errors, filename),
            },
            Err(module_error) => {
                match parse_script_with_recovery(code, filename, Default::default()) {
                    Ok(script) if script.recoverable_errors.is_empty() => {
                        collect_resolved_output_warnings(script.module, filename)
                    }
                    Ok(script) => output_parse_warnings(script.recoverable_errors, filename),
                    Err(_) => vec![UnpackWarning::new(
                        filename,
                        UnpackWarningKind::OutputParseFailed,
                        format!("emitted output failed to parse: {module_error}"),
                    )],
                }
            }
        }
    })
}

fn collect_resolved_output_warnings(
    mut module: swc_core::ecma::ast::Module,
    filename: &str,
) -> Vec<UnpackWarning> {
    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

    let mut warnings = collect_tdz_warnings(&module, filename);
    warnings.extend(collect_duplicate_declaration_warnings(&module, filename));
    warnings
}

fn output_parse_warnings(errors: Vec<ParseDiagnostic>, filename: &str) -> Vec<UnpackWarning> {
    errors
        .into_iter()
        .map(|error| {
            UnpackWarning::new(
                filename,
                UnpackWarningKind::OutputParseRecovered,
                format!("emitted output parse recovered from parser error: {error}"),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_diagnostic(line: usize, message: &str) -> ParseDiagnostic {
        ParseDiagnostic {
            filename: "classic-script.js".to_string(),
            line,
            column: 1,
            message: message.to_string(),
        }
    }

    #[test]
    fn input_parse_warning_coalescing_preserves_signature_order() {
        let warnings = collect_input_parse_warnings(&[
            parse_diagnostic(2, "WithInStrict"),
            parse_diagnostic(3, "TS1102"),
            parse_diagnostic(5, "WithInStrict"),
            parse_diagnostic(8, "TS1102"),
        ]);

        assert_eq!(warnings.len(), 2, "warnings should coalesce: {warnings:#?}");
        assert!(warnings[0].message.contains("WithInStrict"));
        assert!(warnings[0].message.contains("2 occurrences"));
        assert!(warnings[0].message.contains("classic-script.js:2:1"));
        assert!(warnings[1].message.contains("TS1102"));
        assert!(warnings[1].message.contains("2 occurrences"));
        assert!(warnings[1].message.contains("classic-script.js:3:1"));
    }
}
