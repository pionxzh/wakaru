use crate::collections::{HashMap, HashSet};
use std::collections::BTreeMap;

use swc_core::atoms::Atom;
use swc_core::common::{
    sync::Lrc, Globals, Mark, SourceMap, Spanned, SyntaxContext, DUMMY_SP, GLOBALS,
};
use swc_core::ecma::ast::{
    ArrayLit, ArrowFunctionBody, AssignOp, AssignTarget, CallExpr, Callee, Expr, ExprOrSpread,
    ExprStmt, FnExpr, Ident, Lit, MemberExpr, MemberProp, Module, ModuleItem, Number, ObjectLit,
    Pat, Prop, PropName, PropOrSpread, SimpleAssignTarget, Stmt,
};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::utils::replace_ident;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::module_path::relative_import_specifier;
use crate::rules::rename_utils::BindingRename;
use crate::unpacker::surrounding::{SurroundingCode, SurroundingItems};
use crate::unpacker::{
    deconflict_runtime_binding_renames, emit_module_with_source_map, sanitize_relative_path,
    source_fallback_for_stmts, span_byte_range, spans_byte_ranges, BundleFormat, DetectedBundle,
    PreparedModuleAst, UnpackResult, UnpackedModule,
};
use crate::utils::swc_safety::apply_fixer;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ModuleId {
    Numeric(usize),
    Named(String),
}

impl ModuleId {
    fn as_string(&self) -> String {
        match self {
            Self::Numeric(value) => value.to_string(),
            Self::Named(value) => value.clone(),
        }
    }

    fn is_named(&self) -> bool {
        matches!(self, Self::Named(_))
    }

    fn is_numeric(&self) -> bool {
        matches!(self, Self::Numeric(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableDialect {
    Browserify,
    CocosCreator2,
}

enum FactoryParams<'a> {
    Function(&'a [swc_core::ecma::ast::Param]),
    Arrow(&'a [Pat]),
}

struct FactoryModule<'a> {
    id: ModuleId,
    filename: String,
    params: FactoryParams<'a>,
    body_stmts: &'a [Stmt],
    dependencies: HashMap<String, ModuleId>,
    source_span: swc_core::common::Span,
}

pub fn detect_and_extract(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "browserify.js", cm.clone()).ok()?;
        detect_from_module_prepared(&module, cm)?.materialize().ok()
    })
}

pub(super) fn detect_from_module_prepared(
    module: &Module,
    cm: Lrc<SourceMap>,
) -> Option<DetectedBundle> {
    for (idx, item) in module.body.iter().enumerate() {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        let surrounding = SurroundingItems::around(&module.body, idx);

        if let Some(call) = cocos_creator_call(expr) {
            if let Some(result) =
                extract_commonjs_table(call, TableDialect::CocosCreator2, surrounding, cm.clone())
            {
                return Some(result);
            }
        }

        let Some(call) = browserify_call(expr) else {
            continue;
        };
        if let Some(result) =
            extract_commonjs_table(call, TableDialect::Browserify, surrounding, cm.clone())
        {
            return Some(result);
        }
    }
    None
}

fn browserify_call(expr: &Expr) -> Option<&CallExpr> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    matches!(strip_parens(callee), Expr::Call(_)).then_some(call)
}

fn cocos_creator_call(expr: &Expr) -> Option<&CallExpr> {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assign.left else {
        return None;
    };
    if !is_window_require_member(target) {
        return None;
    }

    let Expr::Call(call) = strip_parens(&assign.right) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    matches!(strip_parens(callee), Expr::Fn(_) | Expr::Arrow(_)).then_some(call)
}

fn is_window_require_member(member: &MemberExpr) -> bool {
    matches!(member.obj.as_ref(), Expr::Ident(object) if object.sym.as_ref() == "window")
        && member_prop_name_is(&member.prop, "__require")
}

/// `surrounding` is the input's top-level code around the prelude call.
fn extract_commonjs_table(
    call: &CallExpr,
    dialect: TableDialect,
    surrounding: SurroundingItems<'_>,
    cm: Lrc<SourceMap>,
) -> Option<DetectedBundle> {
    if call.args.len() != 3 {
        return None;
    }

    let Expr::Object(modules_object) = strip_parens(&call.args[0].expr) else {
        return None;
    };
    let Expr::Array(entries_array) = strip_parens(&call.args[2].expr) else {
        return None;
    };
    if dialect == TableDialect::CocosCreator2 {
        let Expr::Object(cache_object) = strip_parens(&call.args[1].expr) else {
            return None;
        };
        if !cache_object.props.is_empty() {
            return None;
        }
    }

    let entry_ids = extract_entry_ids(entries_array)?;
    let mut descriptors = collect_factory_modules(modules_object, dialect)?;
    if descriptors.is_empty() {
        return None;
    }

    let matches_dialect = match dialect {
        TableDialect::Browserify => {
            entry_ids.iter().all(ModuleId::is_numeric)
                && descriptors.iter().all(|module| module.id.is_numeric())
        }
        TableDialect::CocosCreator2 => {
            entry_ids.iter().all(ModuleId::is_named)
                && descriptors.iter().all(|module| module.id.is_named())
                && descriptors.iter().any(has_cocos_registration_markers)
        }
    };
    if !matches_dialect {
        return None;
    }

    // Code around the prelude call runs before or after the bundle; entry.js
    // keeps it, in source order, around the entry modules' requires.
    let surrounding = SurroundingCode::collect(surrounding, &["require"])?;
    assign_filenames(
        &mut descriptors,
        &entry_ids,
        dialect,
        !surrounding.is_empty(),
    );
    let id_to_filename: HashMap<ModuleId, String> = descriptors
        .iter()
        .map(|module| (module.id.clone(), module.filename.clone()))
        .collect();
    if id_to_filename.len() != descriptors.len() {
        return None;
    }
    let entry_set: HashSet<ModuleId> = entry_ids.iter().cloned().collect();

    let mut modules = Vec::with_capacity(descriptors.len() + 1);
    let mut prepared = Vec::with_capacity(descriptors.len() + 1);
    for descriptor in &descriptors {
        let ast = prepare_factory_module(descriptor, &id_to_filename, dialect)?;
        modules.push(UnpackedModule {
            id: descriptor.id.as_string(),
            // With code around the call, entry.js runs the entries.
            is_entry: surrounding.is_empty() && entry_set.contains(&descriptor.id),
            code: source_fallback_for_stmts(&cm, descriptor.body_stmts),
            filename: descriptor.filename.clone(),
            source_ranges: span_byte_range(&cm, descriptor.source_span)
                .into_iter()
                .collect(),
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map: Vec::new(),
            verbatim_source_offset: None,
            mapped_in_every_mode: false,
        });
        prepared.push(Some(ast));
    }
    if !surrounding.is_empty() {
        modules.push(surrounding_entry_module(
            &surrounding,
            &entry_ids,
            &id_to_filename,
            &cm,
        )?);
        prepared.push(None);
    }

    Some(DetectedBundle::new(
        UnpackResult::new(modules, BundleFormat::Browserify),
        prepared,
        cm,
    ))
}

/// entry.js for code around the prelude call: the code before it, a
/// `require` of each entry module in the prelude's order, and the code after
/// it.
fn surrounding_entry_module(
    surrounding: &SurroundingCode,
    entry_ids: &[ModuleId],
    id_to_filename: &HashMap<ModuleId, String>,
    cm: &Lrc<SourceMap>,
) -> Option<UnpackedModule> {
    let mut module = build_module_from_stmts(
        entry_ids
            .iter()
            .map(|id| {
                let specifier =
                    relative_import_specifier(SURROUNDING_ENTRY, id_to_filename.get(id)?);
                Some(Stmt::Expr(ExprStmt {
                    span: DUMMY_SP,
                    expr: Box::new(Expr::Call(CallExpr {
                        callee: Callee::Expr(Box::new(Expr::Ident(Ident::new_no_ctxt(
                            "require".into(),
                            DUMMY_SP,
                        )))),
                        args: vec![Expr::from(specifier.as_str()).into()],
                        ..Default::default()
                    })),
                }))
            })
            .collect::<Option<Vec<_>>>()?,
    );
    surrounding.place_around(&mut module);
    let (code, generated_source_map) = emit_module_with_source_map(&module, cm.clone()).ok()?;
    Some(UnpackedModule {
        id: "entry".to_string(),
        is_entry: true,
        code,
        filename: SURROUNDING_ENTRY.to_string(),
        source_ranges: spans_byte_ranges(cm, surrounding.spans()),
        inspection_context_ranges: Vec::new(),
        source_input: String::new(),
        generated_source_map,
        verbatim_source_offset: None,
        mapped_in_every_mode: false,
    })
}

/// The file holding code around the prelude call. Entry modules then take
/// the multi-entry names (`entry-<id>.js`).
const SURROUNDING_ENTRY: &str = "entry.js";

fn collect_factory_modules(
    modules: &ObjectLit,
    dialect: TableDialect,
) -> Option<Vec<FactoryModule<'_>>> {
    let mut result = Vec::with_capacity(modules.props.len());
    for property in &modules.props {
        let PropOrSpread::Prop(property) = property else {
            return None;
        };
        let Prop::KeyValue(key_value) = property.as_ref() else {
            return None;
        };
        let id = module_id_from_prop_name(&key_value.key, dialect)?;
        let Expr::Array(parts) = strip_parens(&key_value.value) else {
            return None;
        };
        if parts.elems.len() != 2 {
            return None;
        }

        let factory_expr = parts.elems[0].as_ref()?.expr.as_ref();
        let (params, body_stmts) = extract_factory_parts(factory_expr)?;
        let dependencies_expr = parts.elems[1].as_ref()?.expr.as_ref();
        let dependencies = match strip_parens(dependencies_expr) {
            Expr::Object(dependencies) => match extract_dependency_map(dependencies) {
                Some(dependencies) => dependencies,
                None if dialect == TableDialect::Browserify => HashMap::default(),
                None => return None,
            },
            _ if dialect == TableDialect::Browserify => HashMap::default(),
            _ => return None,
        };

        result.push(FactoryModule {
            id,
            filename: String::new(),
            params,
            body_stmts,
            dependencies,
            source_span: key_value.value.span(),
        });
    }
    Some(result)
}

fn module_id_from_prop_name(name: &PropName, dialect: TableDialect) -> Option<ModuleId> {
    match name {
        PropName::Num(Number { value, .. }) if *value >= 0.0 && value.fract() == 0.0 => {
            let value = *value as usize;
            Some(match dialect {
                TableDialect::Browserify => ModuleId::Numeric(value),
                TableDialect::CocosCreator2 => ModuleId::Named(value.to_string()),
            })
        }
        PropName::Str(value) => Some(ModuleId::Named(value.value.as_str()?.to_string())),
        PropName::Ident(value) => Some(ModuleId::Named(value.sym.to_string())),
        _ => None,
    }
}

fn module_id_from_expr(expr: &Expr) -> Option<ModuleId> {
    match strip_parens(expr) {
        Expr::Lit(Lit::Num(Number { value, .. })) if *value >= 0.0 && value.fract() == 0.0 => {
            Some(ModuleId::Numeric(*value as usize))
        }
        Expr::Lit(Lit::Str(value)) => Some(ModuleId::Named(value.value.as_str()?.to_string())),
        _ => None,
    }
}

fn extract_entry_ids(entries: &ArrayLit) -> Option<Vec<ModuleId>> {
    entries
        .elems
        .iter()
        .map(|element| {
            let ExprOrSpread { expr, spread } = element.as_ref()?;
            if spread.is_some() {
                return None;
            }
            module_id_from_expr(expr)
        })
        .collect()
}

fn extract_dependency_map(object: &ObjectLit) -> Option<HashMap<String, ModuleId>> {
    let mut dependencies = HashMap::default();
    for property in &object.props {
        let PropOrSpread::Prop(property) = property else {
            return None;
        };
        let Prop::KeyValue(key_value) = property.as_ref() else {
            return None;
        };
        let request = match &key_value.key {
            PropName::Str(value) => value.value.as_str()?.to_string(),
            PropName::Ident(value) => value.sym.to_string(),
            PropName::Num(Number { value, .. }) if *value >= 0.0 && value.fract() == 0.0 => {
                (*value as usize).to_string()
            }
            _ => return None,
        };
        let target = module_id_from_expr(&key_value.value)?;
        dependencies.insert(request, target);
    }
    Some(dependencies)
}

fn extract_factory_parts(expr: &Expr) -> Option<(FactoryParams<'_>, &[Stmt])> {
    match strip_parens(expr) {
        Expr::Fn(FnExpr { function, .. }) => {
            let body = function.body.as_ref()?;
            Some((FactoryParams::Function(&function.params), &body.stmts))
        }
        Expr::Arrow(arrow) => {
            let ArrowFunctionBody::FunctionBody(body) = arrow.body.as_ref() else {
                return None;
            };
            Some((FactoryParams::Arrow(&arrow.params), &body.stmts))
        }
        _ => None,
    }
}

/// `reserve_entry` keeps `entry.js` for the code around the prelude call.
fn assign_filenames(
    modules: &mut [FactoryModule<'_>],
    entries: &[ModuleId],
    dialect: TableDialect,
    reserve_entry: bool,
) {
    let entry_set: HashSet<&ModuleId> = entries.iter().collect();
    let mut seen = HashSet::default();
    if reserve_entry {
        dedup_filename(SURROUNDING_ENTRY, &mut seen);
    }

    if dialect == TableDialect::CocosCreator2 {
        for module in modules {
            let candidate = named_module_filename(&module.id);
            module.filename = dedup_filename(&candidate, &mut seen);
        }
        return;
    }

    let hints = browserify_filename_hints(modules);

    // Entry names are public/stable and must win collisions with request hints
    // such as `./entry`. Non-entry modules then claim unambiguous readable
    // names in table order, with case-insensitive suffixing.
    for want_entry in [true, false] {
        for module in &mut *modules {
            let is_entry = entry_set.contains(&module.id);
            if is_entry != want_entry {
                continue;
            }
            let id = module.id.as_string();
            let candidate = if is_entry && entries.len() == 1 && !reserve_entry {
                "entry.js".to_string()
            } else if is_entry {
                format!("entry-{id}.js")
            } else {
                hints
                    .get(&module.id)
                    .cloned()
                    .unwrap_or_else(|| format!("module-{id}.js"))
            };
            module.filename = dedup_filename(&candidate, &mut seen);
        }
    }
}

fn browserify_filename_hints(modules: &[FactoryModule<'_>]) -> HashMap<ModuleId, String> {
    let known_ids = modules
        .iter()
        .map(|module| module.id.clone())
        .collect::<HashSet<_>>();
    let mut candidates: HashMap<ModuleId, BTreeMap<String, String>> = HashMap::default();

    for module in modules {
        for (request, target) in &module.dependencies {
            if !known_ids.contains(target) {
                continue;
            }
            let Some(candidate) = browserify_request_filename(request) else {
                continue;
            };
            let case_folded = candidate.to_ascii_lowercase();
            candidates
                .entry(target.clone())
                .or_default()
                .entry(case_folded)
                .and_modify(|existing| {
                    if candidate < *existing {
                        existing.clone_from(&candidate);
                    }
                })
                .or_insert(candidate);
        }
    }

    candidates
        .into_iter()
        .filter_map(|(id, mut names)| {
            (names.len() == 1).then(|| (id, names.pop_first().expect("one filename hint").1))
        })
        .collect()
}

fn browserify_request_filename(request: &str) -> Option<String> {
    if request.parse::<usize>().is_ok() || request.starts_with('/') || request.starts_with('\\') {
        return None;
    }

    let sanitized = sanitize_relative_path(request, "");
    if sanitized.is_empty() {
        return None;
    }
    let lowercase = sanitized.to_ascii_lowercase();
    if [".js", ".mjs", ".cjs"]
        .iter()
        .any(|extension| lowercase.ends_with(extension))
    {
        Some(sanitized)
    } else {
        Some(format!("{sanitized}.js"))
    }
}

fn named_module_filename(id: &ModuleId) -> String {
    let ModuleId::Named(id) = id else {
        return format!("module-{}.js", id.as_string());
    };
    let sanitized = sanitize_relative_path(id, "unknown");
    if [".js", ".mjs", ".cjs"]
        .iter()
        .any(|extension| sanitized.ends_with(extension))
    {
        sanitized
    } else {
        format!("{sanitized}.js")
    }
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

fn has_cocos_registration_markers(module: &FactoryModule<'_>) -> bool {
    let mut push = false;
    let mut pop = false;
    for statement in module.body_stmts {
        let Stmt::Expr(ExprStmt { expr, .. }) = statement else {
            continue;
        };
        push |= top_level_expr_has_cc_rf_call(expr, "push");
        pop |= top_level_expr_has_cc_rf_call(expr, "pop");
    }
    push && pop
}

/// Match direct Cocos registration calls and calls combined into top-level
/// comma sequences by production minifiers. Do not visit call arguments or
/// nested function bodies: registration markers only count at factory scope.
fn top_level_expr_has_cc_rf_call(expression: &Expr, method: &str) -> bool {
    let expression = strip_parens(expression);
    if let Expr::Seq(sequence) = expression {
        return sequence
            .exprs
            .iter()
            .any(|expression| top_level_expr_has_cc_rf_call(expression, method));
    }

    let Expr::Call(call) = expression else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(method_member) = strip_parens(callee) else {
        return false;
    };
    if !member_prop_name_is(&method_member.prop, method) {
        return false;
    }
    let Expr::Member(rf_member) = strip_parens(&method_member.obj) else {
        return false;
    };
    matches!(rf_member.obj.as_ref(), Expr::Ident(object) if object.sym.as_ref() == "cc")
        && member_prop_name_is(&rf_member.prop, "_RF")
}

fn prepare_factory_module(
    descriptor: &FactoryModule<'_>,
    id_to_filename: &HashMap<ModuleId, String>,
    dialect: TableDialect,
) -> Option<PreparedModuleAst> {
    let globals = Globals::new();
    let (module, unresolved_mark) = GLOBALS.set(&globals, || {
        let (mut module, unresolved_mark) =
            normalize_factory_module(descriptor, id_to_filename, dialect)?;
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

fn normalize_factory_module(
    descriptor: &FactoryModule<'_>,
    id_to_filename: &HashMap<ModuleId, String>,
    dialect: TableDialect,
) -> Option<(Module, Mark)> {
    let mut module = build_module_from_stmts(descriptor.body_stmts.to_vec());
    let param_symbols: Vec<Atom> = match descriptor.params {
        FactoryParams::Function(params) => params
            .iter()
            .filter_map(|parameter| match &parameter.pat {
                Pat::Ident(binding) => Some(binding.sym.clone()),
                _ => None,
            })
            .collect(),
        FactoryParams::Arrow(params) => params
            .iter()
            .filter_map(|parameter| match parameter {
                Pat::Ident(binding) => Some(binding.sym.clone()),
                _ => None,
            })
            .collect(),
    };

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

    let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
    let renames = param_symbols
        .iter()
        .zip(["require", "module", "exports"])
        .filter(|(source, target)| source.as_ref() != *target)
        .map(|(source, target)| BindingRename {
            old: (source.clone(), unresolved_ctxt),
            new: target.into(),
        })
        .collect::<Vec<_>>();
    if !deconflict_runtime_binding_renames(&mut module, &renames, None) {
        return None;
    }
    for rename in &renames {
        replace_ident(
            &mut module,
            rename.old.clone(),
            &swc_core::ecma::ast::Ident::new(
                rename.new.clone(),
                Default::default(),
                unresolved_ctxt,
            ),
        );
    }

    module.visit_mut_with(&mut DependencyMapRewriter {
        unresolved_mark,
        from_filename: &descriptor.filename,
        dependencies: &descriptor.dependencies,
        id_to_filename,
        dialect,
    });

    Some((module, unresolved_mark))
}

struct DependencyMapRewriter<'a> {
    unresolved_mark: Mark,
    from_filename: &'a str,
    dependencies: &'a HashMap<String, ModuleId>,
    id_to_filename: &'a HashMap<ModuleId, String>,
    dialect: TableDialect,
}

impl VisitMut for DependencyMapRewriter<'_> {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        call.visit_mut_children_with(self);

        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Ident(require) = strip_parens(callee) else {
            return;
        };
        if require.sym.as_ref() != "require" || require.ctxt.outer() != self.unresolved_mark {
            return;
        }
        if call.args.len() != 1 || call.args[0].spread.is_some() {
            return;
        }

        let Some(request_id) = module_id_from_expr(&call.args[0].expr) else {
            return;
        };
        let filename = match &request_id {
            ModuleId::Named(request) => match self.dependencies.get(request) {
                // An explicit map entry is authoritative. If its target belongs to a
                // different bundle, preserve the original request instead of binding
                // it to an unrelated same-named module in this table.
                Some(id) => self.id_to_filename.get(id),
                None => self.id_to_filename.get(&request_id).or_else(|| {
                    (self.dialect == TableDialect::CocosCreator2)
                        .then(|| cocos_basename_id(request))
                        .flatten()
                        .and_then(|id| self.id_to_filename.get(&id))
                }),
            },
            ModuleId::Numeric(_) => self.id_to_filename.get(&request_id),
        };
        let Some(filename) = filename else {
            return;
        };
        let specifier = relative_import_specifier(self.from_filename, filename);
        let span = call.args[0].expr.span();
        *call.args[0].expr = Expr::Lit(Lit::Str(swc_core::ecma::ast::Str {
            span,
            value: specifier.into(),
            raw: None,
        }));
    }
}

fn cocos_basename_id(request: &str) -> Option<ModuleId> {
    let basename = request.rsplit('/').next()?;
    (!basename.is_empty()).then(|| ModuleId::Named(basename.to_string()))
}

fn build_module_from_stmts(stmts: Vec<Stmt>) -> Module {
    Module {
        span: Default::default(),
        body: stmts.into_iter().map(ModuleItem::Stmt).collect(),
        shebang: None,
    }
}

fn member_prop_name_is(property: &MemberProp, expected: &str) -> bool {
    match property {
        MemberProp::Ident(identifier) => identifier.sym.as_ref() == expected,
        MemberProp::Computed(computed) => {
            matches!(strip_parens(&computed.expr), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some(expected))
        }
        _ => false,
    }
}

fn strip_parens(mut expression: &Expr) -> &Expr {
    while let Expr::Paren(paren) = expression {
        expression = &paren.expr;
    }
    expression
}
