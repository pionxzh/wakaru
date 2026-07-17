use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{
    sync::Lrc, Globals, Mark, SourceMap, Spanned, SyntaxContext, DUMMY_SP, GLOBALS,
};
use swc_core::ecma::ast::*;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;
use crate::module_path::relative_import_specifier;
use crate::rules::rename_utils::{rename_bindings_in_module, BindingRename};
use crate::unpacker::{
    deconflict_runtime_binding_renames, sanitize_relative_path, span_byte_range, BundleFormat,
    DetectedBundle, PreparedModuleAst, UnpackResult, UnpackedModule,
};
use crate::utils::paren::strip_parens;
use crate::utils::swc_safety::apply_fixer;

const DEFINE_SUFFIX: &str = "__d";
const REQUIRE_SUFFIX: &str = "__r";
const FACTORY_PARAM_COUNT: usize = 7;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const NORMALIZED_PARAMS: [&str; FACTORY_PARAM_COUNT] = [
    "global",
    "require",
    "__metroImportDefault",
    "__metroImportAll",
    "module",
    "exports",
    "__metroDependencyMap",
];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum MetroModuleId {
    Numeric(u64),
    String(String),
}

impl MetroModuleId {
    fn display(&self) -> String {
        match self {
            Self::Numeric(value) => value.to_string(),
            Self::String(value) => value.clone(),
        }
    }

    fn module_filename(&self) -> String {
        match self {
            Self::Numeric(value) => format!("module-{value}.js"),
            Self::String(value) => sanitize_relative_path(value, "unknown.js"),
        }
    }

    fn entry_filename(&self, entry_count: usize) -> String {
        if entry_count == 1 {
            return "entry.js".to_string();
        }
        let suffix = sanitize_relative_path(&self.display(), "unknown")
            .replace('/', "-")
            .trim_end_matches(".js")
            .to_string();
        format!("entry-{suffix}.js")
    }
}

struct MetroModuleDescriptor<'a> {
    id: MetroModuleId,
    params: Vec<Atom>,
    body: &'a BlockStmt,
    dependencies: HashMap<usize, Option<MetroModuleId>>,
    dependency_map_expr: Option<Box<Expr>>,
}

#[derive(Clone, Copy)]
enum MetroDependencyKind {
    Require,
    ImportDefault,
    ImportAll,
}

pub fn detect_and_extract(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "metro.js", cm.clone()).ok()?;
        detect_from_module_prepared(&module, cm)?.materialize().ok()
    })
}

pub(super) fn detect_from_module_prepared(
    module: &Module,
    cm: Lrc<SourceMap>,
) -> Option<DetectedBundle> {
    let mut definitions = Vec::new();
    let mut prefix_counts = HashMap::<String, usize>::new();

    for item in &module.body {
        let Some(call) = expression_call(item) else {
            continue;
        };
        let Some(prefix) = callee_prefix(call, DEFINE_SUFFIX) else {
            continue;
        };
        let descriptor = parse_module_definition(call);
        if descriptor.is_some() {
            *prefix_counts.entry(prefix.clone()).or_default() += 1;
        }
        definitions.push((prefix, descriptor));
    }

    let selected_prefix = prefix_counts
        .into_iter()
        .max_by_key(|(prefix, count)| (*count, prefix.is_empty()))?
        .0;
    if definitions
        .iter()
        .any(|(prefix, descriptor)| prefix == &selected_prefix && descriptor.is_none())
    {
        return None;
    }
    let mut descriptors = definitions
        .into_iter()
        .filter_map(|(prefix, descriptor)| {
            (prefix == selected_prefix).then_some(descriptor).flatten()
        })
        .collect::<Vec<_>>();
    if descriptors.is_empty() {
        return None;
    }

    let entry_ids = collect_entry_ids(module, &selected_prefix);
    let known_ids = descriptors
        .iter()
        .map(|descriptor| descriptor.id.clone())
        .collect::<HashSet<_>>();
    let entry_count = entry_ids
        .iter()
        .filter(|id| known_ids.contains(*id))
        .count();

    let mut seen_ids = HashSet::new();
    descriptors.retain(|descriptor| seen_ids.insert(descriptor.id.clone()));

    let filenames = assign_filenames(&descriptors, &entry_ids, entry_count);

    let mut modules = Vec::new();
    let mut prepared = Vec::new();
    for descriptor in descriptors {
        let filename = filenames.get(&descriptor.id)?.clone();
        let is_entry = entry_ids.contains(&descriptor.id);
        let prepared_module = prepare_metro_module(&descriptor, &filename, &filenames)?;
        modules.push(UnpackedModule {
            id: descriptor.id.display(),
            is_entry,
            code: source_fallback_for_body(&cm, descriptor.body),
            filename,
            source_ranges: span_byte_range(&cm, descriptor.body.span)
                .into_iter()
                .collect(),
            source_input: String::new(),
            generated_source_map: Vec::new(),
        });
        prepared.push(Some(prepared_module));
    }

    Some(DetectedBundle::new(
        UnpackResult::new(modules, BundleFormat::Metro),
        prepared,
        cm,
    ))
}

fn assign_filenames(
    descriptors: &[MetroModuleDescriptor<'_>],
    entry_ids: &HashSet<MetroModuleId>,
    entry_count: usize,
) -> HashMap<MetroModuleId, String> {
    let mut filenames = HashMap::with_capacity(descriptors.len());
    let mut seen = HashSet::new();

    // Reserve canonical entry names before ordinary string IDs such as
    // `entry.js`, then make every remaining collision explicit and stable.
    for want_entry in [true, false] {
        for descriptor in descriptors {
            let is_entry = entry_ids.contains(&descriptor.id);
            if is_entry != want_entry {
                continue;
            }
            let candidate = if is_entry {
                descriptor.id.entry_filename(entry_count)
            } else {
                descriptor.id.module_filename()
            };
            filenames.insert(descriptor.id.clone(), dedup_filename(&candidate, &mut seen));
        }
    }
    filenames
}

fn dedup_filename(filename: &str, seen: &mut HashSet<String>) -> String {
    if seen.insert(filename.to_ascii_lowercase()) {
        return filename.to_string();
    }
    let (stem, extension) = filename
        .rsplit_once('.')
        .map(|(stem, extension)| (stem, format!(".{extension}")))
        .unwrap_or((filename, String::new()));
    for suffix in 2.. {
        let candidate = format!("{stem}-{suffix}{extension}");
        if seen.insert(candidate.to_ascii_lowercase()) {
            return candidate;
        }
    }
    unreachable!()
}

fn expression_call(item: &ModuleItem) -> Option<&CallExpr> {
    let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
        return None;
    };
    let Expr::Call(call) = strip_parens(&expr_stmt.expr) else {
        return None;
    };
    Some(call)
}

fn callee_prefix(call: &CallExpr, suffix: &str) -> Option<String> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(ident) = strip_parens(callee) else {
        return None;
    };
    ident.sym.as_ref().strip_suffix(suffix).map(str::to_string)
}

fn parse_module_definition(call: &CallExpr) -> Option<MetroModuleDescriptor<'_>> {
    if !(2..=5).contains(&call.args.len()) {
        return None;
    }
    let (params, body) = factory_parts(&call.args[0].expr)?;
    if params.len() != FACTORY_PARAM_COUNT {
        return None;
    }
    let id = parse_module_id(&call.args[1].expr)?;
    let dependency_map_expr = call.args.get(2).and_then(|arg| {
        matches!(strip_parens(&arg.expr), Expr::Array(_) | Expr::Object(_))
            .then(|| arg.expr.clone())
    });
    let dependencies = match call.args.get(2) {
        Some(arg) => parse_dependency_map(&arg.expr)?,
        None => HashMap::new(),
    };
    Some(MetroModuleDescriptor {
        id,
        params,
        body,
        dependencies,
        dependency_map_expr,
    })
}

fn factory_parts(expr: &Expr) -> Option<(Vec<Atom>, &BlockStmt)> {
    match strip_parens(expr) {
        Expr::Fn(function) => {
            let body = function.function.body.as_ref()?;
            let params = function
                .function
                .params
                .iter()
                .map(|param| pat_ident(&param.pat))
                .collect::<Option<Vec<_>>>()?;
            Some((params, body))
        }
        Expr::Arrow(arrow) => {
            let BlockStmtOrExpr::BlockStmt(body) = &*arrow.body else {
                return None;
            };
            let params = arrow
                .params
                .iter()
                .map(pat_ident)
                .collect::<Option<Vec<_>>>()?;
            Some((params, body))
        }
        _ => None,
    }
}

fn pat_ident(pat: &Pat) -> Option<Atom> {
    let Pat::Ident(binding) = pat else {
        return None;
    };
    Some(binding.sym.clone())
}

fn parse_module_id(expr: &Expr) -> Option<MetroModuleId> {
    match strip_parens(expr) {
        Expr::Lit(Lit::Num(number))
            if number.value.is_finite()
                && number.value >= 0.0
                && number.value.fract() == 0.0
                && number.value <= MAX_SAFE_INTEGER =>
        {
            Some(MetroModuleId::Numeric(number.value as u64))
        }
        Expr::Lit(Lit::Str(string)) => {
            Some(MetroModuleId::String(string.value.as_str()?.to_string()))
        }
        _ => None,
    }
}

fn parse_dependency_map(expr: &Expr) -> Option<HashMap<usize, Option<MetroModuleId>>> {
    match strip_parens(expr) {
        Expr::Array(array) => array
            .elems
            .iter()
            .enumerate()
            .map(|(index, element)| {
                let value = match element {
                    Some(element) if element.spread.is_none() => {
                        parse_dependency_value(&element.expr)?
                    }
                    Some(_) => return None,
                    None => None,
                };
                Some((index, value))
            })
            .collect(),
        Expr::Object(object) => {
            let mut dependencies = HashMap::new();
            for prop in &object.props {
                let PropOrSpread::Prop(prop) = prop else {
                    return None;
                };
                let Prop::KeyValue(key_value) = &**prop else {
                    continue;
                };
                let Some(index) = dependency_index_from_prop_name(&key_value.key) else {
                    continue;
                };
                dependencies.insert(index, parse_dependency_value(&key_value.value)?);
            }
            Some(dependencies)
        }
        Expr::Ident(ident) if ident.sym == *"undefined" => Some(HashMap::new()),
        _ => None,
    }
}

fn parse_dependency_value(expr: &Expr) -> Option<Option<MetroModuleId>> {
    if matches!(strip_parens(expr), Expr::Lit(Lit::Null(_))) {
        return Some(None);
    }
    parse_module_id(expr).map(Some)
}

fn dependency_index_from_prop_name(name: &PropName) -> Option<usize> {
    match name {
        PropName::Num(number)
            if number.value >= 0.0
                && number.value.fract() == 0.0
                && number.value <= usize::MAX as f64 =>
        {
            Some(number.value as usize)
        }
        PropName::Str(string) => string.value.as_str()?.parse().ok(),
        PropName::Ident(ident) => ident.sym.parse().ok(),
        _ => None,
    }
}

fn collect_entry_ids(module: &Module, prefix: &str) -> HashSet<MetroModuleId> {
    let prefixed_callee = format!("{prefix}{REQUIRE_SUFFIX}");
    module
        .body
        .iter()
        .filter_map(expression_call)
        .filter(|call| {
            let Callee::Expr(callee) = &call.callee else {
                return false;
            };
            matches!(strip_parens(callee), Expr::Ident(ident)
                if ident.sym == *REQUIRE_SUFFIX || ident.sym.as_ref() == prefixed_callee)
        })
        .filter_map(|call| call.args.first())
        .filter_map(|arg| parse_module_id(&arg.expr))
        .collect()
}

fn prepare_metro_module(
    descriptor: &MetroModuleDescriptor<'_>,
    filename: &str,
    filenames: &HashMap<MetroModuleId, String>,
) -> Option<PreparedModuleAst> {
    let globals = Globals::new();
    let (module, unresolved_mark) = GLOBALS.set(&globals, || {
        let (mut module, unresolved_mark) =
            normalize_metro_module(descriptor, filename, filenames)?;
        apply_fixer(&mut module).ok()?;
        Some((module, unresolved_mark))
    })?;
    Some(PreparedModuleAst {
        globals,
        module,
        unresolved_mark,
        recoverable_parse_errors: Vec::new(),
    })
}

fn normalize_metro_module(
    descriptor: &MetroModuleDescriptor<'_>,
    filename: &str,
    filenames: &HashMap<MetroModuleId, String>,
) -> Option<(Module, Mark)> {
    let mut module = Module {
        span: DUMMY_SP,
        body: descriptor
            .body
            .stmts
            .iter()
            .cloned()
            .map(ModuleItem::Stmt)
            .collect(),
        shebang: None,
    };
    if let Some(init) = &descriptor.dependency_map_expr {
        module.body.insert(
            0,
            dependency_map_declaration(&descriptor.params[FACTORY_PARAM_COUNT - 1], init.clone()),
        );
    }

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

    let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
    let dependency_map_ctxt = if descriptor.dependency_map_expr.is_some() {
        dependency_map_binding(&module)?.1
    } else {
        unresolved_ctxt
    };

    let renames = descriptor
        .params
        .iter()
        .zip(NORMALIZED_PARAMS)
        .enumerate()
        .filter(|(_, (old_sym, target))| old_sym.as_ref() != *target)
        .map(|(index, (old_sym, target))| BindingRename {
            old: (
                old_sym.clone(),
                if index == FACTORY_PARAM_COUNT - 1 {
                    dependency_map_ctxt
                } else {
                    unresolved_ctxt
                },
            ),
            new: target.into(),
        })
        .collect::<Vec<_>>();
    if !deconflict_runtime_binding_renames(&mut module, &renames) {
        return None;
    }
    rename_bindings_in_module(&mut module, &renames);

    let mut dependency_rewriter = MetroDependencyRewriter {
        unresolved_mark,
        dependency_map_ctxt,
        from_filename: filename,
        filenames,
        dependencies: &descriptor.dependencies,
    };
    rewrite_metro_import_declarations(&mut module, &dependency_rewriter);
    module.visit_mut_with(&mut dependency_rewriter);

    let mut reference_finder = MetroDependencyMapRefFinder {
        ctxt: dependency_map_ctxt,
        found: false,
    };
    module.visit_with(&mut reference_finder);
    if reference_finder.found {
        descriptor.dependency_map_expr.as_ref()?;
    } else if descriptor.dependency_map_expr.is_some() {
        remove_dependency_map_declaration(&mut module, dependency_map_ctxt)?;
    }

    Some((module, unresolved_mark))
}

struct MetroDependencyRewriter<'a> {
    unresolved_mark: Mark,
    dependency_map_ctxt: SyntaxContext,
    from_filename: &'a str,
    filenames: &'a HashMap<MetroModuleId, String>,
    dependencies: &'a HashMap<usize, Option<MetroModuleId>>,
}

impl VisitMut for MetroDependencyRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        let Expr::Call(call) = expr else {
            return;
        };
        let Some((kind, dependency)) = self.dependency_for_call(call) else {
            return;
        };

        let dependency_expr = match dependency {
            Some(id) => Expr::Lit(Lit::Str(Str {
                span: DUMMY_SP,
                value: self.specifier_for_id(id).into(),
                raw: None,
            })),
            None => Expr::Lit(Lit::Null(Null { span: DUMMY_SP })),
        };

        let mut require_call = call.clone();
        require_call.callee = Callee::Expr(Box::new(Expr::Ident(Ident::new(
            "require".into(),
            DUMMY_SP,
            SyntaxContext::empty().apply_mark(self.unresolved_mark),
        ))));
        require_call.args = vec![ExprOrSpread {
            spread: None,
            expr: Box::new(dependency_expr),
        }];

        // Import-loader calls normally became declarations above. Keep a
        // source-like fallback for uncommon expression-position calls.
        *expr = match kind {
            MetroDependencyKind::Require | MetroDependencyKind::ImportAll => {
                Expr::Call(require_call)
            }
            MetroDependencyKind::ImportDefault => Expr::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Call(require_call)),
                prop: MemberProp::Ident(IdentName::new("default".into(), DUMMY_SP)),
            }),
        };
    }
}

impl MetroDependencyRewriter<'_> {
    fn dependency_for_call(
        &self,
        call: &CallExpr,
    ) -> Option<(MetroDependencyKind, &Option<MetroModuleId>)> {
        let kind = self.dependency_kind(call)?;
        let index = call
            .args
            .first()
            .and_then(|arg| dependency_index_from_expr(arg, self.dependency_map_ctxt))?;
        self.dependencies
            .get(&index)
            .map(|dependency| (kind, dependency))
    }

    fn dependency_kind(&self, call: &CallExpr) -> Option<MetroDependencyKind> {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Ident(ident) = strip_parens(callee) else {
            return None;
        };
        if ident.ctxt.outer() != self.unresolved_mark {
            return None;
        }
        match ident.sym.as_ref() {
            "require" => Some(MetroDependencyKind::Require),
            "__metroImportDefault" => Some(MetroDependencyKind::ImportDefault),
            "__metroImportAll" => Some(MetroDependencyKind::ImportAll),
            _ => None,
        }
    }

    fn specifier_for_id(&self, id: &MetroModuleId) -> String {
        let target = self
            .filenames
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.module_filename());
        relative_import_specifier(self.from_filename, &target)
    }
}

fn rewrite_metro_import_declarations(module: &mut Module, rewriter: &MetroDependencyRewriter<'_>) {
    // Metro passes distinct runtime functions for ESM default and namespace
    // imports. That provenance is stronger than a plain `require()` shape, so
    // recover the import kind while the factory parameter identity is available.
    let binding_uses = BindingUseIndex::collect(module);
    let mut items = Vec::with_capacity(module.body.len());
    for item in std::mem::take(&mut module.body) {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            items.push(item);
            continue;
        };

        let mut remaining = Vec::new();
        for declarator in var.decls.iter().cloned() {
            let Some(import) = metro_import_from_declarator(&declarator, rewriter, &binding_uses)
            else {
                remaining.push(declarator);
                continue;
            };
            if !remaining.is_empty() {
                let mut remaining_var = var.clone();
                remaining_var.decls = std::mem::take(&mut remaining);
                items.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(remaining_var))));
            }
            items.push(ModuleItem::ModuleDecl(ModuleDecl::Import(import)));
        }
        if !remaining.is_empty() {
            let mut remaining_var = var;
            remaining_var.decls = remaining;
            items.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(remaining_var))));
        }
    }
    module.body = items;
}

fn metro_import_from_declarator(
    declarator: &VarDeclarator,
    rewriter: &MetroDependencyRewriter<'_>,
    binding_uses: &BindingUseIndex,
) -> Option<ImportDecl> {
    let Pat::Ident(binding) = &declarator.name else {
        return None;
    };
    let binding_id = (binding.id.sym.clone(), binding.id.ctxt);
    if binding_uses.has_direct_write(&binding_id) {
        return None;
    }
    let Expr::Call(call) = strip_parens(declarator.init.as_ref()?) else {
        return None;
    };
    let (kind, dependency) = rewriter.dependency_for_call(call)?;
    let id = dependency.as_ref()?;
    let local = Ident::new(binding.id.sym.clone(), DUMMY_SP, binding.id.ctxt);
    let specifier = match kind {
        MetroDependencyKind::Require => return None,
        MetroDependencyKind::ImportDefault => ImportSpecifier::Default(ImportDefaultSpecifier {
            span: DUMMY_SP,
            local,
        }),
        MetroDependencyKind::ImportAll => ImportSpecifier::Namespace(ImportStarAsSpecifier {
            span: DUMMY_SP,
            local,
        }),
    };
    Some(ImportDecl {
        span: DUMMY_SP,
        specifiers: vec![specifier],
        src: Box::new(Str {
            span: DUMMY_SP,
            value: rewriter.specifier_for_id(id).into(),
            raw: None,
        }),
        type_only: false,
        with: None,
        phase: Default::default(),
    })
}

fn dependency_index_from_expr(
    arg: &ExprOrSpread,
    dependency_map_ctxt: SyntaxContext,
) -> Option<usize> {
    if arg.spread.is_some() {
        return None;
    }
    let Expr::Member(member) = strip_parens(&arg.expr) else {
        return None;
    };
    let Expr::Ident(object) = strip_parens(&member.obj) else {
        return None;
    };
    if object.sym != *"__metroDependencyMap" || object.ctxt != dependency_map_ctxt {
        return None;
    }
    let MemberProp::Computed(computed) = &member.prop else {
        return None;
    };
    match strip_parens(&computed.expr) {
        Expr::Lit(Lit::Num(number))
            if number.value >= 0.0
                && number.value.fract() == 0.0
                && number.value <= usize::MAX as f64 =>
        {
            Some(number.value as usize)
        }
        Expr::Lit(Lit::Str(string)) => string.value.as_str()?.parse().ok(),
        _ => None,
    }
}

fn dependency_map_declaration(sym: &Atom, init: Box<Expr>) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: Ident::new(sym.clone(), DUMMY_SP, SyntaxContext::empty()),
                type_ann: None,
            }),
            init: Some(init),
            definite: false,
        }],
    }))))
}

fn dependency_map_binding(module: &Module) -> Option<(Atom, SyntaxContext)> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = module.body.first()? else {
        return None;
    };
    let Pat::Ident(binding) = &var.decls.first()?.name else {
        return None;
    };
    Some((binding.id.sym.clone(), binding.id.ctxt))
}

fn remove_dependency_map_declaration(
    module: &mut Module,
    dependency_map_ctxt: SyntaxContext,
) -> Option<()> {
    let binding = dependency_map_binding(module)?;
    if binding.0 != *"__metroDependencyMap" || binding.1 != dependency_map_ctxt {
        return None;
    }
    module.body.remove(0);
    Some(())
}

struct MetroDependencyMapRefFinder {
    ctxt: SyntaxContext,
    found: bool,
}

impl Visit for MetroDependencyMapRefFinder {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == *"__metroDependencyMap" && ident.ctxt == self.ctxt {
            self.found = true;
        }
    }

    fn visit_binding_ident(&mut self, _: &BindingIdent) {}
}

fn source_fallback_for_body(cm: &SourceMap, body: &BlockStmt) -> String {
    let (Some(first), Some(last)) = (body.stmts.first(), body.stmts.last()) else {
        return String::new();
    };
    let file = cm.lookup_byte_offset(first.span_lo()).sf;
    let start = first.span_lo().0.saturating_sub(file.start_pos.0) as usize;
    let end = last.span_hi().0.saturating_sub(file.start_pos.0) as usize;
    file.src.get(start..end).unwrap_or_default().to_string()
}
