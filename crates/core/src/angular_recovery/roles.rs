use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    CallExpr, Callee, Expr, ImportSpecifier, Module, ModuleDecl, ModuleExportName, ModuleItem,
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

    fn export_name(self) -> &'static str {
        match self {
            Self::DefineComponent => "ɵɵdefineComponent",
            Self::ElementStart => "ɵɵelementStart",
            Self::ElementEnd => "ɵɵelementEnd",
            Self::Element => "ɵɵelement",
            Self::Text => "ɵɵtext",
            Self::Listener => "ɵɵlistener",
            Self::Advance => "ɵɵadvance",
            Self::TextInterpolate => "ɵɵtextInterpolate",
            Self::TextInterpolate1 => "ɵɵtextInterpolate1",
            Self::TextInterpolate2 => "ɵɵtextInterpolate2",
            Self::TextInterpolate3 => "ɵɵtextInterpolate3",
            Self::TextInterpolate4 => "ɵɵtextInterpolate4",
            Self::TextInterpolate5 => "ɵɵtextInterpolate5",
            Self::TextInterpolate6 => "ɵɵtextInterpolate6",
            Self::TextInterpolate7 => "ɵɵtextInterpolate7",
            Self::TextInterpolate8 => "ɵɵtextInterpolate8",
            Self::Property => "ɵɵproperty",
            Self::Attribute => "ɵɵattribute",
            Self::ClassProp => "ɵɵclassProp",
            Self::StyleProp => "ɵɵstyleProp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SymbolIdentity {
    Binding(BindingKey),
    Member { object: BindingKey, property: Atom },
}

#[derive(Default)]
pub(super) struct IvyRoleTable {
    roles: HashMap<SymbolIdentity, IvyInstruction>,
    ivy_names: HashMap<SymbolIdentity, String>,
    core_namespaces: HashSet<BindingKey>,
}

impl IvyRoleTable {
    pub(super) fn collect(modules: &[PreparedAngularModule]) -> Self {
        let mut table = Self::default();
        for prepared in modules {
            table.collect_imports(&prepared.module);
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
                            self.ivy_names.insert(
                                SymbolIdentity::Binding(binding_key(&named.local)),
                                imported.clone(),
                            );
                        }
                        if let Some(role) = IvyInstruction::from_export_name(&imported) {
                            self.roles
                                .insert(SymbolIdentity::Binding(binding_key(&named.local)), role);
                        }
                    }
                    ImportSpecifier::Default(_) => {}
                }
            }
        }
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
        if let Some(identity) = symbol_identity(expr) {
            if let Some(name) = self.ivy_names.get(&identity) {
                return Some(name.clone());
            }
            if let Some(role) = self.roles.get(&identity) {
                return Some(role.export_name().to_string());
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
    pub(super) define_component_calls: Vec<CallExpr>,
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
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self
            .roles
            .instruction_for_callee(&call.callee, self.unresolved_ctxt)
            == Some(IvyInstruction::DefineComponent)
        {
            self.define_component_calls.push(call.clone());
        }
        call.visit_children_with(self);
    }
}

fn symbol_identity(expr: &Expr) -> Option<SymbolIdentity> {
    match expr {
        Expr::Ident(ident) => Some(SymbolIdentity::Binding(binding_key(ident))),
        Expr::Member(member) => {
            let Expr::Ident(object) = member.obj.as_ref() else {
                return None;
            };
            Some(SymbolIdentity::Member {
                object: binding_key(object),
                property: member_prop_name(&member.prop)?,
            })
        }
        _ => None,
    }
}

fn module_export_name(name: &ModuleExportName) -> String {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.to_string(),
        ModuleExportName::Str(string) => wtf8_to_string(&string.value),
    }
}
