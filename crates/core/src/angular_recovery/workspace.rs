use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Spanned, SyntaxContext};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, CallExpr, Callee, Decl, DefaultDecl, ExportSpecifier, Expr,
    ExprOrSpread, ForHead, ForInStmt, ForOfStmt, ImportSpecifier, MemberProp, ModuleDecl,
    ModuleExportName, ModuleItem, ObjectPatProp, Pat, SimpleAssignTarget, UnaryExpr, UnaryOp,
    UpdateExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;

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

#[derive(Default)]
pub(super) struct WorkspaceAliasIndex {
    group_by_symbol: HashMap<WorkspaceSymbol, usize>,
}

impl WorkspaceAliasIndex {
    pub(super) fn collect(modules: &[PreparedAngularModule]) -> Self {
        let aliases = collect_esm_symbol_aliases(modules);
        let mut adjacency = HashMap::<WorkspaceSymbol, Vec<WorkspaceSymbol>>::new();
        for alias in aliases {
            adjacency
                .entry(alias.left.clone())
                .or_default()
                .push(alias.right.clone());
            adjacency.entry(alias.right).or_default().push(alias.left);
        }

        let mut index = Self::default();
        let mut visited = HashSet::new();
        for start in adjacency.keys() {
            if visited.contains(start) {
                continue;
            }
            let group = index.group_by_symbol.len();
            let mut stack = vec![start.clone()];
            while let Some(symbol) = stack.pop() {
                if !visited.insert(symbol.clone()) {
                    continue;
                }
                if let Some(neighbors) = adjacency.get(&symbol) {
                    stack.extend(neighbors.iter().cloned());
                }
                index.group_by_symbol.insert(symbol, group);
            }
        }
        index
    }

    pub(super) fn group(&self, symbol: &WorkspaceSymbol) -> Option<usize> {
        self.group_by_symbol.get(symbol).copied()
    }
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
            let mut matches = [".js", ".jsx", ".mjs", ".cjs"]
                .into_iter()
                .filter_map(|extension| self.filenames.get(&format!("{filename}{extension}")))
                .copied();
            let resolved = matches.next();
            if resolved.is_some() && matches.next().is_none() {
                return resolved;
            }
            if resolved.is_some() {
                return None;
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
    let binding_uses = BindingUseIndex::collect(module);
    let mut namespace_mutations = NamespaceMutationCollector {
        unresolved_ctxt,
        paths: HashMap::new(),
    };
    module.visit_with(&mut namespace_mutations);

    let mut collector = ImmediateIifeAliasCollector {
        unresolved_ctxt,
        aliases: HashMap::new(),
        ambiguous: HashSet::new(),
    };
    module.visit_with(&mut collector);
    collector.aliases.retain(|binding, alias| {
        !binding_uses.has_direct_write(binding)
            && namespace_path(alias.expression.as_ref(), unresolved_ctxt).is_some_and(|path| {
                !namespace_mutations
                    .paths
                    .iter()
                    .any(|(mutation, positions)| {
                        path.starts_with(mutation)
                            && positions
                                .iter()
                                .any(|position| *position >= alias.invocation_position)
                    })
            })
    });
    if collector.aliases.is_empty() {
        return;
    }

    module.visit_mut_with(&mut NamespaceAliasRewriter {
        aliases: collector
            .aliases
            .into_iter()
            .map(|(binding, alias)| (binding, alias.expression))
            .collect(),
    });
}

type NamespacePath = Vec<Atom>;

struct NamespaceMutationCollector {
    unresolved_ctxt: SyntaxContext,
    paths: HashMap<NamespacePath, Vec<u32>>,
}

impl NamespaceMutationCollector {
    fn record_expression(&mut self, expression: &Expr) {
        if let Some(path) = namespace_path(expression, self.unresolved_ctxt) {
            self.paths
                .entry(path)
                .or_default()
                .push(expression.span().lo.0);
        }
    }

    fn record_simple_target(&mut self, target: &SimpleAssignTarget) {
        match target {
            SimpleAssignTarget::Ident(binding) => {
                self.record_expression(&Expr::Ident(binding.id.clone()));
            }
            SimpleAssignTarget::Member(member) => {
                self.record_expression(&Expr::Member(member.clone()));
            }
            SimpleAssignTarget::Paren(paren) => self.record_expression(paren.expr.as_ref()),
            SimpleAssignTarget::TsAs(ts_as) => self.record_expression(ts_as.expr.as_ref()),
            SimpleAssignTarget::TsSatisfies(ts_satisfies) => {
                self.record_expression(ts_satisfies.expr.as_ref());
            }
            SimpleAssignTarget::TsNonNull(ts_non_null) => {
                self.record_expression(ts_non_null.expr.as_ref());
            }
            SimpleAssignTarget::TsTypeAssertion(ts_assertion) => {
                self.record_expression(ts_assertion.expr.as_ref());
            }
            SimpleAssignTarget::TsInstantiation(ts_instantiation) => {
                self.record_expression(ts_instantiation.expr.as_ref());
            }
            _ => {}
        }
    }

    fn record_pattern(&mut self, pattern: &Pat) {
        match pattern {
            Pat::Ident(binding) => {
                self.record_expression(&Expr::Ident(binding.id.clone()));
            }
            Pat::Array(array) => {
                for element in array.elems.iter().flatten() {
                    self.record_pattern(element);
                }
            }
            Pat::Object(object) => {
                for property in &object.props {
                    match property {
                        ObjectPatProp::KeyValue(key_value) => {
                            self.record_pattern(key_value.value.as_ref());
                        }
                        ObjectPatProp::Assign(assign) => {
                            self.record_expression(&Expr::Ident(assign.key.id.clone()));
                        }
                        ObjectPatProp::Rest(rest) => self.record_pattern(rest.arg.as_ref()),
                    }
                }
            }
            Pat::Assign(assign) => self.record_pattern(assign.left.as_ref()),
            Pat::Rest(rest) => self.record_pattern(rest.arg.as_ref()),
            Pat::Expr(expression) => self.record_expression(expression.as_ref()),
            Pat::Invalid(_) => {}
        }
    }

    fn record_assignment_target(&mut self, target: &AssignTarget) {
        match target {
            AssignTarget::Simple(simple) => self.record_simple_target(simple),
            AssignTarget::Pat(pattern) => match pattern {
                swc_core::ecma::ast::AssignTargetPat::Array(array) => {
                    for element in array.elems.iter().flatten() {
                        self.record_pattern(element);
                    }
                }
                swc_core::ecma::ast::AssignTargetPat::Object(object) => {
                    self.record_pattern(&Pat::Object(object.clone()));
                }
                swc_core::ecma::ast::AssignTargetPat::Invalid(_) => {}
            },
        }
    }

    fn record_for_head(&mut self, head: &ForHead) {
        if let ForHead::Pat(pattern) = head {
            self.record_pattern(pattern);
        }
    }
}

impl Visit for NamespaceMutationCollector {
    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        self.record_assignment_target(&assignment.left);
        assignment.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        self.record_expression(update.arg.as_ref());
        update.visit_children_with(self);
    }

    fn visit_for_in_stmt(&mut self, statement: &ForInStmt) {
        self.record_for_head(&statement.left);
        statement.visit_children_with(self);
    }

    fn visit_for_of_stmt(&mut self, statement: &ForOfStmt) {
        self.record_for_head(&statement.left);
        statement.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::Delete {
            self.record_expression(unary.arg.as_ref());
        }
        unary.visit_children_with(self);
    }
}

struct ImmediateIifeAliasCollector {
    unresolved_ctxt: SyntaxContext,
    aliases: HashMap<BindingKey, ImmediateIifeAlias>,
    ambiguous: HashSet<BindingKey>,
}

struct ImmediateIifeAlias {
    expression: Box<Expr>,
    invocation_position: u32,
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
                    .is_some_and(|existing| existing.expression.as_ref() != argument.expr.as_ref())
                {
                    self.aliases.remove(&key);
                    self.ambiguous.insert(key);
                    continue;
                }
                self.aliases.insert(
                    key,
                    ImmediateIifeAlias {
                        expression: argument.expr.clone(),
                        invocation_position: call.span.lo.0,
                    },
                );
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
    namespace_path(expression, unresolved_ctxt).is_some()
}

fn namespace_path(expression: &Expr, unresolved_ctxt: SyntaxContext) -> Option<NamespacePath> {
    match expression {
        Expr::This(_) => Some(vec![Atom::from("this")]),
        Expr::Ident(identifier) if identifier.ctxt == unresolved_ctxt => {
            Some(vec![identifier.sym.clone()])
        }
        Expr::Member(member) => {
            let mut path = namespace_path(member.obj.as_ref(), unresolved_ctxt)?;
            path.push(member_prop_name(&member.prop)?);
            Some(path)
        }
        Expr::Paren(paren) => namespace_path(paren.expr.as_ref(), unresolved_ctxt),
        _ => None,
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
    fn extensionless_resolution_rejects_ambiguous_module_candidates() {
        let lookup = ModuleLookup {
            filenames: HashMap::from([
                ("dependency.js".to_string(), 0),
                ("dependency.mjs".to_string(), 1),
            ]),
        };

        assert_eq!(lookup.resolve_normalized("dependency"), None);
        assert_eq!(lookup.resolve_normalized("dependency.js"), Some(0));
        assert_eq!(lookup.resolve_normalized("dependency.mjs"), Some(1));
    }

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

    #[test]
    fn does_not_canonicalize_a_destructured_parameter_write() {
        let module = canonicalized_fixture(
            "(function(namespace) { [namespace] = sources; namespace.value(); })(this.shared);",
        );

        let mut finder = NamespaceUseFinder::default();
        module.visit_with(&mut finder);
        assert!(finder.local_namespace_use);
        assert!(!finder.global_namespace_use);
    }

    #[test]
    fn does_not_canonicalize_a_loop_head_parameter_write() {
        let module = canonicalized_fixture(
            "(function(namespace) { for (namespace of sources) {} namespace.value(); })(this.shared);",
        );

        let mut finder = NamespaceUseFinder::default();
        module.visit_with(&mut finder);
        assert!(finder.local_namespace_use);
        assert!(!finder.global_namespace_use);
    }

    #[test]
    fn does_not_canonicalize_a_namespace_member_that_is_later_reassigned() {
        let module = canonicalized_fixture(
            "(function(namespace) { namespace.value(); })(shared.current); shared.current = other;",
        );

        let mut finder = NamespaceUseFinder::default();
        module.visit_with(&mut finder);
        assert!(finder.local_namespace_use);
    }

    #[test]
    fn canonicalizes_a_namespace_member_initialized_before_the_iife() {
        let module = canonicalized_fixture(
            "this.shared = this.shared || {}; \
             (function(namespace) { namespace.value(); }).call(this, this.shared);",
        );

        let mut finder = NamespaceUseFinder::default();
        module.visit_with(&mut finder);
        assert!(!finder.local_namespace_use);
        assert!(finder.global_namespace_use);
    }

    fn canonicalized_fixture(source: &str) -> swc_core::ecma::ast::Module {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(
                FileName::Custom("fixture.js".to_string()).into(),
                source.to_string(),
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
            module
        })
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
