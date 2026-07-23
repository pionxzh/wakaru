use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, CallExpr, Callee, Decl, DefaultDecl, ExportSpecifier, Expr,
    ExprOrSpread, ImportSpecifier, MemberProp, ModuleDecl, ModuleExportName, ModuleItem, Pat,
    SimpleAssignTarget, UpdateExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::syntax::{binding_key, member_prop_name, BindingKey};
use super::PreparedAngularModule;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum WorkspaceSymbol {
    Binding(BindingKey),
    Member { object: BindingKey, property: Atom },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkspaceSymbolAlias {
    pub(super) left: WorkspaceSymbol,
    pub(super) right: WorkspaceSymbol,
}

/// Collect symbol equivalences expressed by ordinary ESM imports and exports.
///
/// This records module transport only: it does not assign framework meaning to
/// either endpoint. Role analyzers can project their own semantic facts across
/// these proven edges.
pub(super) fn collect_esm_symbol_aliases(
    modules: &[PreparedAngularModule],
) -> Vec<WorkspaceSymbolAlias> {
    let lookup = ModuleLookup::new(modules);
    let exports = modules
        .iter()
        .map(|module| collect_local_exports(&module.module))
        .collect::<Vec<_>>();
    let mut aliases = Vec::new();

    for module in modules {
        for item in &module.module.body {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                continue;
            };
            if import.type_only {
                continue;
            }
            let source = import
                .src
                .value
                .as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| import.src.value.to_string_lossy().into_owned());
            let Some(target_index) = lookup.resolve(&module.filename, &source) else {
                continue;
            };

            for specifier in &import.specifiers {
                match specifier {
                    ImportSpecifier::Default(default) => {
                        record_import_alias(
                            &mut aliases,
                            WorkspaceSymbol::Binding(binding_key(&default.local)),
                            "default",
                            &exports[target_index],
                        );
                    }
                    ImportSpecifier::Named(named) => {
                        let imported = named
                            .imported
                            .as_ref()
                            .map(module_export_name)
                            .unwrap_or_else(|| named.local.sym.to_string());
                        record_import_alias(
                            &mut aliases,
                            WorkspaceSymbol::Binding(binding_key(&named.local)),
                            &imported,
                            &exports[target_index],
                        );
                    }
                    ImportSpecifier::Namespace(namespace) => {
                        let object = binding_key(&namespace.local);
                        for (exported, target) in &exports[target_index] {
                            aliases.push(WorkspaceSymbolAlias {
                                left: WorkspaceSymbol::Member {
                                    object: object.clone(),
                                    property: Atom::from(exported.as_str()),
                                },
                                right: target.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    aliases
}

fn record_import_alias(
    aliases: &mut Vec<WorkspaceSymbolAlias>,
    local: WorkspaceSymbol,
    imported: &str,
    exports: &HashMap<String, WorkspaceSymbol>,
) {
    let Some(target) = exports.get(imported) else {
        return;
    };
    aliases.push(WorkspaceSymbolAlias {
        left: local,
        right: target.clone(),
    });
}

fn collect_local_exports(module: &swc_core::ecma::ast::Module) -> HashMap<String, WorkspaceSymbol> {
    let mut exports = HashMap::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(declaration) = item else {
            continue;
        };
        match declaration {
            ModuleDecl::ExportDecl(export) => match &export.decl {
                Decl::Class(class) => {
                    exports.insert(
                        class.ident.sym.to_string(),
                        WorkspaceSymbol::Binding(binding_key(&class.ident)),
                    );
                }
                Decl::Fn(function) => {
                    exports.insert(
                        function.ident.sym.to_string(),
                        WorkspaceSymbol::Binding(binding_key(&function.ident)),
                    );
                }
                Decl::Var(variable) => {
                    for declaration in &variable.decls {
                        if let Pat::Ident(binding) = &declaration.name {
                            exports.insert(
                                binding.id.sym.to_string(),
                                WorkspaceSymbol::Binding(binding_key(&binding.id)),
                            );
                        }
                    }
                }
                _ => {}
            },
            ModuleDecl::ExportNamed(named) if named.src.is_none() => {
                for specifier in &named.specifiers {
                    let ExportSpecifier::Named(named) = specifier else {
                        continue;
                    };
                    let ModuleExportName::Ident(local) = &named.orig else {
                        continue;
                    };
                    let exported = named
                        .exported
                        .as_ref()
                        .map(module_export_name)
                        .unwrap_or_else(|| local.sym.to_string());
                    exports.insert(exported, WorkspaceSymbol::Binding(binding_key(local)));
                }
            }
            ModuleDecl::ExportDefaultDecl(default) => {
                let local = match &default.decl {
                    DefaultDecl::Class(class) => class.ident.as_ref(),
                    DefaultDecl::Fn(function) => function.ident.as_ref(),
                    DefaultDecl::TsInterfaceDecl(_) => None,
                };
                if let Some(local) = local {
                    exports.insert(
                        "default".to_string(),
                        WorkspaceSymbol::Binding(binding_key(local)),
                    );
                }
            }
            ModuleDecl::ExportDefaultExpr(default) => {
                if let Expr::Ident(local) = default.expr.as_ref() {
                    exports.insert(
                        "default".to_string(),
                        WorkspaceSymbol::Binding(binding_key(local)),
                    );
                }
            }
            _ => {}
        }
    }
    exports
}

fn module_export_name(name: &ModuleExportName) -> String {
    match name {
        ModuleExportName::Ident(identifier) => identifier.sym.to_string(),
        ModuleExportName::Str(string) => string
            .value
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| string.value.to_string_lossy().into_owned()),
    }
}

struct ModuleLookup {
    filenames: HashMap<String, usize>,
}

impl ModuleLookup {
    fn new(modules: &[PreparedAngularModule]) -> Self {
        let filenames = modules
            .iter()
            .enumerate()
            .map(|(index, module)| (normalize_filename(&module.filename), index))
            .collect();
        Self { filenames }
    }

    fn resolve(&self, from_filename: &str, specifier: &str) -> Option<usize> {
        let resolved = crate::module_path::resolve_relative_specifier(from_filename, specifier)?;
        self.resolve_normalized(&resolved)
    }

    fn resolve_normalized(&self, filename: &str) -> Option<usize> {
        let filename = normalize_filename(filename);
        if let Some(index) = self.filenames.get(&filename) {
            return Some(*index);
        }
        if !has_module_extension(&filename) {
            for extension in [".js", ".jsx", ".mjs", ".cjs"] {
                if let Some(index) = self.filenames.get(&format!("{filename}{extension}")) {
                    return Some(*index);
                }
            }
        }
        for extension in [".js", ".jsx", ".mjs", ".cjs"] {
            if let Some(stem) = filename.strip_suffix(extension) {
                if let Some(index) = self.filenames.get(stem) {
                    return Some(*index);
                }
            }
        }
        None
    }
}

fn normalize_filename(filename: &str) -> String {
    let normalized = filename.replace('\\', "/");
    normalized
        .strip_prefix("./")
        .unwrap_or(&normalized)
        .to_string()
}

fn has_module_extension(filename: &str) -> bool {
    [".js", ".jsx", ".mjs", ".cjs"]
        .iter()
        .any(|extension| filename.ends_with(extension))
}

/// Canonicalize stable namespace arguments passed into immediately invoked
/// functions. This is generic module-workspace normalization: it does not
/// depend on a bundle format or assign any Ivy meaning.
pub(super) fn canonicalize_immediate_iife_namespace_aliases(
    module: &mut swc_core::ecma::ast::Module,
    unresolved_ctxt: SyntaxContext,
) {
    let mut writes = BindingWriteCollector::default();
    module.visit_with(&mut writes);

    let mut collector = ImmediateIifeAliasCollector {
        unresolved_ctxt,
        aliases: HashMap::new(),
        ambiguous: HashSet::new(),
    };
    module.visit_with(&mut collector);
    collector
        .aliases
        .retain(|binding, _| !writes.bindings.contains(binding));
    if collector.aliases.is_empty() {
        return;
    }

    module.visit_mut_with(&mut NamespaceAliasRewriter {
        aliases: collector.aliases,
    });
}

#[derive(Default)]
struct BindingWriteCollector {
    bindings: HashSet<BindingKey>,
}

impl Visit for BindingWriteCollector {
    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assignment.left {
            self.bindings.insert(binding_key(&binding.id));
        }
        assignment.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if let Expr::Ident(identifier) = update.arg.as_ref() {
            self.bindings.insert(binding_key(identifier));
        }
        update.visit_children_with(self);
    }
}

struct ImmediateIifeAliasCollector {
    unresolved_ctxt: SyntaxContext,
    aliases: HashMap<BindingKey, Box<Expr>>,
    ambiguous: HashSet<BindingKey>,
}

impl Visit for ImmediateIifeAliasCollector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some((parameters, arguments)) = invoked_function_parameters(call) {
            for (parameter, argument) in parameters.into_iter().zip(arguments) {
                let Pat::Ident(binding) = parameter else {
                    continue;
                };
                if argument.spread.is_some()
                    || !is_stable_namespace_expression(argument.expr.as_ref(), self.unresolved_ctxt)
                {
                    continue;
                }
                let key = binding_key(&binding.id);
                if self.ambiguous.contains(&key) {
                    continue;
                }
                if self
                    .aliases
                    .get(&key)
                    .is_some_and(|existing| existing.as_ref() != argument.expr.as_ref())
                {
                    self.aliases.remove(&key);
                    self.ambiguous.insert(key);
                    continue;
                }
                self.aliases.insert(key, argument.expr.clone());
            }
        }
        call.visit_children_with(self);
    }
}

fn invoked_function_parameters(call: &CallExpr) -> Option<(Vec<&Pat>, &[ExprOrSpread])> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if let Some(parameters) = function_parameters(callee.as_ref()) {
        return Some((parameters, &call.args));
    }

    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    if !matches!(
        &member.prop,
        MemberProp::Ident(property) if property.sym.as_ref() == "call"
    ) {
        return None;
    }
    let parameters = function_parameters(member.obj.as_ref())?;
    Some((parameters, call.args.get(1..)?))
}

fn function_parameters(expression: &Expr) -> Option<Vec<&Pat>> {
    match expression {
        Expr::Fn(function) => Some(
            function
                .function
                .params
                .iter()
                .map(|parameter| &parameter.pat)
                .collect(),
        ),
        Expr::Arrow(arrow) => Some(arrow.params.iter().collect()),
        Expr::Paren(paren) => function_parameters(paren.expr.as_ref()),
        _ => None,
    }
}

fn is_stable_namespace_expression(expression: &Expr, unresolved_ctxt: SyntaxContext) -> bool {
    match expression {
        Expr::This(_) => true,
        Expr::Ident(identifier) => identifier.ctxt == unresolved_ctxt,
        Expr::Member(member) => {
            member_prop_name(&member.prop).is_some()
                && is_stable_namespace_expression(member.obj.as_ref(), unresolved_ctxt)
        }
        Expr::Paren(paren) => is_stable_namespace_expression(paren.expr.as_ref(), unresolved_ctxt),
        _ => false,
    }
}

struct NamespaceAliasRewriter {
    aliases: HashMap<BindingKey, Box<Expr>>,
}

impl VisitMut for NamespaceAliasRewriter {
    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        expression.visit_mut_children_with(self);
        let Expr::Ident(identifier) = expression else {
            return;
        };
        let Some(replacement) = self.aliases.get(&binding_key(identifier)) else {
            return;
        };
        *expression = replacement.as_ref().clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
    use swc_core::ecma::transforms::base::resolver;

    #[test]
    fn canonicalizes_unwritten_immediate_iife_namespace_parameters() {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(
                FileName::Custom("fixture.js".to_string()).into(),
                "(function(namespace) { namespace.value(); }).call(this, this.shared);".to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*file),
                None,
            );
            let mut module = Parser::new_from(lexer)
                .parse_module()
                .expect("fixture should parse");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            canonicalize_immediate_iife_namespace_aliases(
                &mut module,
                SyntaxContext::empty().apply_mark(unresolved_mark),
            );

            let mut finder = NamespaceUseFinder::default();
            module.visit_with(&mut finder);
            assert!(!finder.local_namespace_use);
            assert!(finder.global_namespace_use);
        });
    }

    #[test]
    fn does_not_canonicalize_a_reassigned_parameter() {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(
                FileName::Custom("fixture.js".to_string()).into(),
                "(function(namespace) { namespace = other; namespace.value(); })(this.shared);"
                    .to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*file),
                None,
            );
            let mut module = Parser::new_from(lexer)
                .parse_module()
                .expect("fixture should parse");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            canonicalize_immediate_iife_namespace_aliases(
                &mut module,
                SyntaxContext::empty().apply_mark(unresolved_mark),
            );

            let mut finder = NamespaceUseFinder::default();
            module.visit_with(&mut finder);
            assert!(finder.local_namespace_use);
            assert!(!finder.global_namespace_use);
        });
    }

    #[derive(Default)]
    struct NamespaceUseFinder {
        local_namespace_use: bool,
        global_namespace_use: bool,
    }

    impl Visit for NamespaceUseFinder {
        fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
            if member_prop_name(&member.prop).is_some_and(|property| property.as_ref() == "value") {
                match member.obj.as_ref() {
                    Expr::Ident(identifier) if identifier.sym.as_ref() == "namespace" => {
                        self.local_namespace_use = true;
                    }
                    Expr::Member(object)
                        if member_prop_name(&object.prop)
                            .is_some_and(|property| property.as_ref() == "shared") =>
                    {
                        self.global_namespace_use = true;
                    }
                    _ => {}
                }
            }
            member.visit_children_with(self);
        }
    }
}
