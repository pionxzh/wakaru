use std::collections::{HashMap, HashSet, VecDeque};

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    BindingIdent, Class, ClassDecl, Decl, Expr, Ident, ImportSpecifier, Module, ModuleDecl,
    ModuleItem, Pat, Stmt, VarDecl,
};

use super::syntax::{binding_key, BindingKey};
use crate::analysis::binding_uses::BindingUseIndex;

type SourceOrder = (usize, usize);

#[derive(Clone)]
enum SupportEntryKind {
    Supported { item: ModuleItem, is_import: bool },
    Unsupported,
}

#[derive(Clone)]
struct SupportEntry {
    order: SourceOrder,
    references: HashSet<BindingKey>,
    kind: SupportEntryKind,
}

#[derive(Default)]
pub(super) struct ArtifactSymbolTable {
    entries: HashMap<BindingKey, SupportEntry>,
}

#[derive(Clone)]
struct SupportUnit {
    binding: BindingKey,
    order: SourceOrder,
    is_import: bool,
    item: ModuleItem,
}

#[derive(Default)]
pub(super) struct ArtifactSupportPlan {
    units: Vec<SupportUnit>,
    unresolved_symbols: HashSet<String>,
    available_roots: HashSet<BindingKey>,
}

impl ArtifactSymbolTable {
    pub(super) fn collect(module: &Module) -> Self {
        let mut table = Self::default();
        for (item_index, item) in module.body.iter().enumerate() {
            match item {
                ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                    for (specifier_index, specifier) in import.specifiers.iter().enumerate() {
                        let binding = import_specifier_binding(specifier);
                        let mut filtered = import.clone();
                        filtered.specifiers = vec![specifier.clone()];
                        table.entries.insert(
                            binding,
                            SupportEntry {
                                order: (item_index, specifier_index),
                                references: HashSet::new(),
                                kind: SupportEntryKind::Supported {
                                    item: ModuleItem::ModuleDecl(ModuleDecl::Import(filtered)),
                                    is_import: true,
                                },
                            },
                        );
                    }
                }
                ModuleItem::Stmt(Stmt::Decl(declaration)) => {
                    table.record_declaration(declaration, item_index);
                }
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                    table.record_declaration(&export.decl, item_index);
                }
                _ => {}
            }
        }
        table
    }

    pub(super) fn recover(
        &self,
        roots: &HashSet<BindingKey>,
        reserved_names: &HashSet<Atom>,
        report_unresolved: bool,
    ) -> ArtifactSupportPlan {
        self.recover_with_provided(roots, reserved_names, &HashSet::new(), report_unresolved)
    }

    pub(super) fn recover_with_provided(
        &self,
        roots: &HashSet<BindingKey>,
        reserved_names: &HashSet<Atom>,
        provided_bindings: &HashSet<BindingKey>,
        report_unresolved: bool,
    ) -> ArtifactSupportPlan {
        let mut plan = ArtifactSupportPlan::default();
        let mut roots = roots
            .iter()
            .filter(|root| self.entries.contains_key(*root) && !provided_bindings.contains(*root))
            .cloned()
            .collect::<Vec<_>>();
        roots.sort_by(|left, right| left.0.cmp(&right.0));

        let mut selected = HashMap::<BindingKey, SupportUnit>::new();
        for root in roots {
            let Some(units) = self.recover_root(&root, reserved_names, provided_bindings) else {
                if report_unresolved {
                    plan.unresolved_symbols.insert(root.0.to_string());
                }
                continue;
            };
            plan.available_roots.insert(root);
            for unit in units {
                selected.entry(unit.binding.clone()).or_insert(unit);
            }
        }

        plan.units = selected.into_values().collect();
        plan.units.sort_by(|left, right| {
            (!left.is_import)
                .cmp(&(!right.is_import))
                .then_with(|| left.order.cmp(&right.order))
        });
        for unit in &plan.units {
            plan.unresolved_symbols.remove(unit.binding.0.as_ref());
        }
        plan
    }

    fn recover_root(
        &self,
        root: &BindingKey,
        reserved_names: &HashSet<Atom>,
        provided_bindings: &HashSet<BindingKey>,
    ) -> Option<Vec<SupportUnit>> {
        let mut pending = VecDeque::from([root.clone()]);
        let mut visited = HashSet::new();
        let mut units = Vec::new();

        while let Some(binding) = pending.pop_front() {
            if !visited.insert(binding.clone()) {
                continue;
            }
            if provided_bindings.contains(&binding) {
                continue;
            }
            let Some(entry) = self.entries.get(&binding) else {
                continue;
            };
            if reserved_names.contains(&binding.0) {
                return None;
            }
            let SupportEntryKind::Supported { item, is_import } = &entry.kind else {
                return None;
            };
            units.push(SupportUnit {
                binding,
                order: entry.order,
                is_import: *is_import,
                item: item.clone(),
            });
            for reference in &entry.references {
                if self.entries.contains_key(reference) {
                    pending.push_back(reference.clone());
                }
            }
        }

        Some(units)
    }

    fn record_declaration(&mut self, declaration: &Decl, item_index: usize) {
        match declaration {
            Decl::Fn(function) => {
                let item = ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function.clone())));
                self.entries.insert(
                    binding_key(&function.ident),
                    SupportEntry {
                        order: (item_index, 0),
                        references: item_references(&item),
                        kind: SupportEntryKind::Supported {
                            item,
                            is_import: false,
                        },
                    },
                );
            }
            Decl::Class(class) => {
                self.entries.insert(
                    binding_key(&class.ident),
                    SupportEntry {
                        order: (item_index, 0),
                        references: HashSet::new(),
                        kind: SupportEntryKind::Unsupported,
                    },
                );
            }
            Decl::Var(variable) => {
                for (declarator_index, declarator) in variable.decls.iter().enumerate() {
                    let Pat::Ident(binding) = &declarator.name else {
                        continue;
                    };
                    let supported = declarator
                        .init
                        .as_deref()
                        .is_some_and(is_portable_initializer);
                    if !supported {
                        self.entries.insert(
                            binding_key(&binding.id),
                            SupportEntry {
                                order: (item_index, declarator_index),
                                references: HashSet::new(),
                                kind: SupportEntryKind::Unsupported,
                            },
                        );
                        continue;
                    }
                    let mut filtered: Box<VarDecl> = variable.clone();
                    filtered.decls = vec![declarator.clone()];
                    let item = ModuleItem::Stmt(Stmt::Decl(Decl::Var(filtered)));
                    self.entries.insert(
                        binding_key(&binding.id),
                        SupportEntry {
                            order: (item_index, declarator_index),
                            references: item_references(&item),
                            kind: SupportEntryKind::Supported {
                                item,
                                is_import: false,
                            },
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

impl ArtifactSupportPlan {
    pub(super) fn merge(&mut self, other: Self) {
        let mut provided_names = self
            .units
            .iter()
            .map(|unit| unit.binding.0.clone())
            .collect::<HashSet<_>>();
        for unit in other.units {
            if provided_names.insert(unit.binding.0.clone()) {
                self.units.push(unit);
            }
        }
        self.unresolved_symbols.extend(other.unresolved_symbols);
        self.available_roots.extend(other.available_roots);
        self.units.sort_by(|left, right| {
            (!left.is_import)
                .cmp(&(!right.is_import))
                .then_with(|| left.order.cmp(&right.order))
        });
        for name in provided_names {
            self.unresolved_symbols.remove(name.as_ref());
        }
    }

    pub(super) fn module_items(&self) -> Vec<ModuleItem> {
        self.units.iter().map(|unit| unit.item.clone()).collect()
    }

    pub(super) fn unresolved_symbols(&self) -> Vec<String> {
        let mut symbols = self.unresolved_symbols.iter().cloned().collect::<Vec<_>>();
        symbols.sort();
        symbols
    }

    pub(super) fn provides(&self, binding: &BindingKey) -> bool {
        self.available_roots.contains(binding)
    }
}

pub(super) fn class_references(class: &Class) -> HashSet<BindingKey> {
    let item = ModuleItem::Stmt(Stmt::Decl(Decl::Class(ClassDecl {
        ident: Ident::new(
            Atom::from("__wakaru_component"),
            DUMMY_SP,
            SyntaxContext::empty(),
        ),
        declare: false,
        class: Box::new(class.clone()),
    })));
    item_references(&item)
}

pub(super) fn expression_references(expression: &Expr) -> HashSet<BindingKey> {
    let item = ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: swc_core::ecma::ast::VarDeclKind::Const,
        declare: false,
        decls: vec![swc_core::ecma::ast::VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: Ident::new(
                    Atom::from("__wakaru_expression"),
                    DUMMY_SP,
                    SyntaxContext::empty(),
                ),
                type_ann: None,
            }),
            init: Some(Box::new(expression.clone())),
            definite: false,
        }],
    }))));
    item_references(&item)
}

pub(super) fn dependency_binding(expression: &Expr) -> Option<BindingKey> {
    match expression {
        Expr::Ident(identifier) => Some(binding_key(identifier)),
        Expr::Paren(parenthesized) => dependency_binding(parenthesized.expr.as_ref()),
        _ => None,
    }
}

fn item_references(item: &ModuleItem) -> HashSet<BindingKey> {
    BindingUseIndex::collect_module_items(std::slice::from_ref(item)).referenced_bindings()
}

fn import_specifier_binding(specifier: &ImportSpecifier) -> BindingKey {
    match specifier {
        ImportSpecifier::Named(named) => binding_key(&named.local),
        ImportSpecifier::Default(default) => binding_key(&default.local),
        ImportSpecifier::Namespace(namespace) => binding_key(&namespace.local),
    }
}

fn is_portable_initializer(expression: &Expr) -> bool {
    match expression {
        Expr::Fn(_) | Expr::Arrow(_) | Expr::Lit(_) | Expr::Ident(_) => true,
        Expr::Paren(parenthesized) => is_portable_initializer(parenthesized.expr.as_ref()),
        _ => false,
    }
}
