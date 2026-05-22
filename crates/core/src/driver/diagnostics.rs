use std::collections::HashMap;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};
use swc_core::ecma::ast::{
    BindingIdent, ClassDecl, ForInStmt, ForOfStmt, ForStmt, ImportDecl, ImportSpecifier,
    ObjectPatProp, Pat, VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::io::{parse_js_with_recovery, ParseDiagnostic};
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
    errors
        .iter()
        .map(|error| {
            UnpackWarning::new(
                &error.filename,
                UnpackWarningKind::InputParseRecovered,
                format!("input parse recovered from parser error: {error}"),
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

pub(super) fn verify_output_parses(code: &str, filename: &str) -> Vec<UnpackWarning> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        match parse_js_with_recovery(code, filename, cm) {
            Ok(parsed) => parsed
                .recoverable_errors
                .into_iter()
                .map(|error| {
                    UnpackWarning::new(
                        filename,
                        UnpackWarningKind::OutputParseRecovered,
                        format!("emitted output parse recovered from parser error: {error}"),
                    )
                })
                .collect(),
            Err(e) => vec![UnpackWarning::new(
                filename,
                UnpackWarningKind::OutputParseFailed,
                format!("emitted output failed to parse: {e}"),
            )],
        }
    })
}
