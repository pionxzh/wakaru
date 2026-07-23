use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, BlockStmtOrExpr, CallExpr, Callee, ClassProp, Expr, ImportSpecifier,
    Module, ModuleDecl, ModuleExportName, ModuleItem, Prop, PropName, SimpleAssignTarget, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::syntax::{binding_key, member_prop_name, wtf8_to_string, BindingKey};
use super::PreparedAngularModule;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum IvyInstruction {
    DefineComponent,
    ElementStart,
    ElementEnd,
    Element,
    Text,
    Listener,
    Advance,
    TextInterpolate,
    TextInterpolate1,
    TextInterpolate2,
    TextInterpolate3,
    TextInterpolate4,
    TextInterpolate5,
    TextInterpolate6,
    TextInterpolate7,
    TextInterpolate8,
    Property,
    Attribute,
    ClassProp,
    StyleProp,
}

impl IvyInstruction {
    fn from_export_name(name: &str) -> Option<Self> {
        Some(match name {
            "ɵɵdefineComponent" => Self::DefineComponent,
            "ɵɵelementStart" => Self::ElementStart,
            "ɵɵelementEnd" => Self::ElementEnd,
            "ɵɵelement" => Self::Element,
            "ɵɵtext" => Self::Text,
            "ɵɵlistener" => Self::Listener,
            "ɵɵadvance" => Self::Advance,
            "ɵɵtextInterpolate" => Self::TextInterpolate,
            "ɵɵtextInterpolate1" => Self::TextInterpolate1,
            "ɵɵtextInterpolate2" => Self::TextInterpolate2,
            "ɵɵtextInterpolate3" => Self::TextInterpolate3,
            "ɵɵtextInterpolate4" => Self::TextInterpolate4,
            "ɵɵtextInterpolate5" => Self::TextInterpolate5,
            "ɵɵtextInterpolate6" => Self::TextInterpolate6,
            "ɵɵtextInterpolate7" => Self::TextInterpolate7,
            "ɵɵtextInterpolate8" => Self::TextInterpolate8,
            "ɵɵproperty" => Self::Property,
            "ɵɵattribute" => Self::Attribute,
            "ɵɵclassProp" => Self::ClassProp,
            "ɵɵstyleProp" => Self::StyleProp,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SymbolIdentity {
    LocalBinding(BindingKey),
    LocalMember { object: BindingKey, property: Atom },
    GlobalBinding(Atom),
    GlobalMember { object: Atom, property: Atom },
}

#[derive(Default)]
pub(super) struct IvyRoleTable {
    ivy_names: HashMap<SymbolIdentity, String>,
    ambiguous_symbols: HashSet<SymbolIdentity>,
    core_namespaces: HashSet<BindingKey>,
}

impl IvyRoleTable {
    pub(super) fn collect(modules: &[PreparedAngularModule]) -> Self {
        let mut table = Self::default();
        for prepared in modules {
            table.collect_imports(&prepared.module);
        }
        for prepared in modules {
            table.collect_export_maps(&prepared.module, prepared.unresolved_ctxt);
        }
        table
    }

    fn collect_imports(&mut self, module: &Module) {
        for item in &module.body {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                continue;
            };
            if wtf8_to_string(&import.src.value) != "@angular/core" {
                continue;
            }
            for specifier in &import.specifiers {
                match specifier {
                    ImportSpecifier::Namespace(namespace) => {
                        self.core_namespaces.insert(binding_key(&namespace.local));
                    }
                    ImportSpecifier::Named(named) => {
                        let imported = named
                            .imported
                            .as_ref()
                            .map(module_export_name)
                            .unwrap_or_else(|| named.local.sym.to_string());
                        if imported.starts_with("ɵɵ") {
                            self.record_mapping(
                                SymbolIdentity::LocalBinding(binding_key(&named.local)),
                                imported,
                            );
                        }
                    }
                    ImportSpecifier::Default(_) => {}
                }
            }
        }
    }

    fn collect_export_maps(&mut self, module: &Module, unresolved_ctxt: SyntaxContext) {
        let mut collector = IvyExportMapCollector {
            unresolved_ctxt,
            mappings: Vec::new(),
        };
        module.visit_with(&mut collector);
        for (identity, name) in collector.mappings {
            self.record_mapping(identity, name);
        }
    }

    fn record_mapping(&mut self, identity: SymbolIdentity, name: String) {
        if self.ambiguous_symbols.contains(&identity) {
            return;
        }
        if self
            .ivy_names
            .get(&identity)
            .is_some_and(|existing| existing != &name)
        {
            self.ivy_names.remove(&identity);
            self.ambiguous_symbols.insert(identity);
            return;
        }
        self.ivy_names.insert(identity, name);
    }

    pub(super) fn instruction_for_callee(
        &self,
        callee: &Callee,
        unresolved_ctxt: SyntaxContext,
    ) -> Option<IvyInstruction> {
        let Callee::Expr(expr) = callee else {
            return None;
        };
        self.instruction_for_expr(expr.as_ref(), unresolved_ctxt)
    }

    pub(super) fn instruction_for_expr(
        &self,
        expr: &Expr,
        unresolved_ctxt: SyntaxContext,
    ) -> Option<IvyInstruction> {
        self.ivy_name_for_expr(expr, unresolved_ctxt)
            .and_then(|name| IvyInstruction::from_export_name(&name))
    }

    pub(super) fn ivy_name_for_expr(
        &self,
        expr: &Expr,
        unresolved_ctxt: SyntaxContext,
    ) -> Option<String> {
        if let Some(identity) = symbol_identity(expr, unresolved_ctxt) {
            if let Some(name) = self.ivy_names.get(&identity) {
                return Some(name.clone());
            }
        }

        match expr {
            Expr::Ident(ident) if ident.ctxt == unresolved_ctxt => {
                ident.sym.starts_with("ɵɵ").then(|| ident.sym.to_string())
            }
            Expr::Member(member) => {
                let Expr::Ident(object) = member.obj.as_ref() else {
                    return None;
                };
                if !self.core_namespaces.contains(&binding_key(object)) {
                    return None;
                }
                let name = member_prop_name(&member.prop)?;
                name.starts_with("ɵɵ").then(|| name.to_string())
            }
            _ => None,
        }
    }
}

pub(super) struct IvyCallCollector<'a> {
    roles: &'a IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    pub(super) define_component_calls: Vec<IvyComponentCall>,
}

pub(super) struct IvyComponentCall {
    pub(super) call: CallExpr,
    pub(super) definition_field: Option<Atom>,
}

impl<'a> IvyCallCollector<'a> {
    pub(super) fn new(roles: &'a IvyRoleTable, unresolved_ctxt: SyntaxContext) -> Self {
        Self {
            roles,
            unresolved_ctxt,
            define_component_calls: Vec::new(),
        }
    }
}

impl<'a> Visit for IvyCallCollector<'a> {
    fn visit_class_prop(&mut self, property: &ClassProp) {
        let Some(Expr::Call(call)) = property.value.as_deref() else {
            property.visit_children_with(self);
            return;
        };
        if !property.is_static || !self.is_define_component(call) {
            property.visit_children_with(self);
            return;
        }
        self.define_component_calls.push(IvyComponentCall {
            call: call.clone(),
            definition_field: prop_name_atom(&property.key),
        });
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        let Expr::Call(call) = assignment.right.as_ref() else {
            assignment.visit_children_with(self);
            return;
        };
        if !self.is_define_component(call) {
            assignment.visit_children_with(self);
            return;
        }
        let definition_field = match &assignment.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                member_prop_name(&member.prop)
            }
            _ => None,
        };
        self.define_component_calls.push(IvyComponentCall {
            call: call.clone(),
            definition_field,
        });
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self.is_define_component(call) {
            self.define_component_calls.push(IvyComponentCall {
                call: call.clone(),
                definition_field: None,
            });
        }
        call.visit_children_with(self);
    }
}

impl IvyCallCollector<'_> {
    fn is_define_component(&self, call: &CallExpr) -> bool {
        self.roles
            .instruction_for_callee(&call.callee, self.unresolved_ctxt)
            == Some(IvyInstruction::DefineComponent)
    }
}

fn prop_name_atom(name: &PropName) -> Option<Atom> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.clone()),
        PropName::Str(string) => Some(Atom::from(wtf8_to_string(&string.value))),
        _ => None,
    }
}

fn symbol_identity(expr: &Expr, unresolved_ctxt: SyntaxContext) -> Option<SymbolIdentity> {
    match expr {
        Expr::Ident(ident) if ident.ctxt == unresolved_ctxt => {
            Some(SymbolIdentity::GlobalBinding(ident.sym.clone()))
        }
        Expr::Ident(ident) => Some(SymbolIdentity::LocalBinding(binding_key(ident))),
        Expr::Member(member) => {
            let Expr::Ident(object) = member.obj.as_ref() else {
                return None;
            };
            let property = member_prop_name(&member.prop)?;
            if object.ctxt == unresolved_ctxt {
                Some(SymbolIdentity::GlobalMember {
                    object: object.sym.clone(),
                    property,
                })
            } else {
                Some(SymbolIdentity::LocalMember {
                    object: binding_key(object),
                    property,
                })
            }
        }
        _ => None,
    }
}

struct IvyExportMapCollector {
    unresolved_ctxt: SyntaxContext,
    mappings: Vec<(SymbolIdentity, String)>,
}

impl Visit for IvyExportMapCollector {
    fn visit_prop(&mut self, prop: &Prop) {
        let Prop::KeyValue(key_value) = prop else {
            prop.visit_children_with(self);
            return;
        };
        let Some(name) = ivy_export_prop_name(&key_value.key) else {
            key_value.visit_children_with(self);
            return;
        };
        if !name.starts_with("ɵɵ") {
            key_value.visit_children_with(self);
            return;
        }
        let Some(value) = exported_symbol_expr(key_value.value.as_ref()) else {
            key_value.visit_children_with(self);
            return;
        };
        if let Some(identity) = symbol_identity(value, self.unresolved_ctxt) {
            self.mappings.push((identity, name));
        }
        key_value.visit_children_with(self);
    }
}

fn ivy_export_prop_name(name: &PropName) -> Option<String> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.to_string()),
        PropName::Str(string) => Some(wtf8_to_string(&string.value)),
        _ => None,
    }
}

fn exported_symbol_expr(expression: &Expr) -> Option<&Expr> {
    match expression {
        Expr::Ident(_) | Expr::Member(_) => Some(expression),
        Expr::Arrow(arrow) if arrow.params.is_empty() => match arrow.body.as_ref() {
            BlockStmtOrExpr::Expr(expression) => exported_symbol_expr(expression),
            BlockStmtOrExpr::BlockStmt(block) => {
                let [Stmt::Return(return_statement)] = block.stmts.as_slice() else {
                    return None;
                };
                exported_symbol_expr(return_statement.arg.as_deref()?)
            }
        },
        Expr::Fn(function) if function.function.params.is_empty() => {
            let [Stmt::Return(return_statement)] =
                function.function.body.as_ref()?.stmts.as_slice()
            else {
                return None;
            };
            exported_symbol_expr(return_statement.arg.as_deref()?)
        }
        Expr::Paren(paren) => exported_symbol_expr(paren.expr.as_ref()),
        _ => None,
    }
}

fn module_export_name(name: &ModuleExportName) -> String {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.to_string(),
        ModuleExportName::Str(string) => wtf8_to_string(&string.value),
    }
}
