//! Turbopack production chunks (Next.js 15.3 and later).
//!
//! A client chunk registers its factories through
//! `(globalThis.TURBOPACK || (globalThis.TURBOPACK = [])).push([script, ...])`
//! (the global may be a computed `globalThis["TURBOPACK_…"]` member); a server
//! chunk assigns the same payload with `module.exports = [...]`. From Next.js
//! 15.5 the payload is runs of numeric module ids, each followed by one
//! factory that the runtime calls as `factory(ctx, module, exports)`.
//!
//! Next.js 15.3–15.4 used `(G = G || []).push([script, { id: factory }])` and
//! `module.exports = { id: factory }`, with factories that take only `ctx`,
//! destructure `m`/`e` (module/exports) from it, and register ESM exports
//! with an object of getters. Those differences are normalized here before
//! the shared translation.
//!
//! Factories are translated into webpack's `(module, exports, require)`
//! calling convention and then prepared by the webpack 5 normalizer, so the
//! existing webpack ESM/CommonJS recovery applies unchanged. Only `ctx`
//! members with a known meaning are translated; any other use keeps that
//! factory opaque. Runtime-defined letters changed meaning across releases,
//! so an unknown member is never guessed.

use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{
    sync::Lrc, FileName, Globals, Mark, SourceMap, Span, Spanned, SyntaxContext, DUMMY_SP, GLOBALS,
};
use swc_core::ecma::ast::{
    ArrayLit, ArrowExpr, ArrowFunctionBody, AssignExpr, AssignOp, AssignTarget, BindingIdent,
    CallExpr, Callee, ClassDecl, ComputedPropName, Expr, ExprOrSpread, ExprStmt, FnDecl, Ident,
    IdentName, KeyValueProp, Lit, MemberExpr, MemberProp, Module, ModuleItem, Number, ObjectLit,
    Pat, Prop, PropName, PropOrSpread, ReturnStmt, SimpleAssignTarget, Stmt, Str,
};
use swc_core::ecma::parser::{Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::js_names::is_valid_identifier_name;
use crate::rules::rename_utils::{rename_bindings_in_module, BindingRename};
use crate::unpacker::webpack5::{prepare_translated_webpack_factories, TranslatedWebpackFactory};
use crate::unpacker::webpack_common::{numeric_id_from_expr, unique_webpack_module_filenames};
use crate::unpacker::{
    source_fallback_for_stmts, spans_byte_ranges, BundleFormat, DetectedBundle,
    DetectedModuleFailure, DetectedModuleNote, PreparedModuleAst, TurbopackContextUse,
    UnpackResult, UnpackedModule,
};
use crate::utils::paren::{strip_parens, strip_parens_mut};

pub(super) fn detect_from_module_prepared(
    module: &Module,
    cm: Lrc<SourceMap>,
) -> Option<DetectedBundle> {
    let span = tracing::info_span!("turbopack: detect_from_module");
    let _enter = span.enter();

    let Chunk { entries, prelude } = collect_entries(module, &cm)?;
    if entries.is_empty() {
        return None;
    }

    let mut seen_ids = HashSet::default();
    for entry in &entries {
        for id in &entry.ids {
            if !seen_ids.insert(*id) {
                // The runtime keeps the first factory for a repeated id; a
                // chunk that repeats one is not a shape we have observed.
                return None;
            }
        }
    }

    let loaders: HashMap<usize, LoaderTarget> = entries
        .iter()
        .filter_map(|entry| Some((entry.ids[0], loader_target(entry.factory)?)))
        .collect();
    let groups = merged_groups(&entries, &seen_ids);
    // Extra ids of a factory that registers exports only for itself are
    // instances of the same module; in a merged group each id is its own
    // module.
    let aliases: HashMap<usize, usize> = entries
        .iter()
        .zip(&groups)
        .filter(|(_, group)| matches!(group, Ok(None)))
        .flat_map(|(entry, _)| entry.ids[1..].iter().map(|alias| (*alias, entry.ids[0])))
        .collect();

    let modules_entries: Vec<&Entry<'_>> = entries.iter().collect();
    let facade_ids: Vec<usize> = groups
        .iter()
        .flatten()
        .flatten()
        .flat_map(|group| group.foreign.iter().map(|facade| facade.id))
        .collect();
    let ids: Vec<String> = modules_entries
        .iter()
        .map(|entry| entry.ids[0])
        .chain(facade_ids.iter().copied())
        .map(|id| id.to_string())
        .collect();
    let mut filenames = unique_webpack_module_filenames(ids.iter().map(String::as_str));
    let facade_filenames: HashMap<usize, String> = facade_ids
        .iter()
        .copied()
        .zip(filenames.split_off(modules_entries.len()))
        .collect();
    let mut ids = ids;
    ids.truncate(modules_entries.len());

    let mut translated = Vec::new();
    let mut translated_index = Vec::with_capacity(modules_entries.len());
    let mut facades = Vec::new();
    let mut failures = HashMap::default();
    let mut notes = HashMap::default();
    for (((entry, id), filename), group) in modules_entries
        .iter()
        .zip(&ids)
        .zip(&filenames)
        .zip(&groups)
    {
        let translation = match (loaders.get(&entry.ids[0]), group) {
            (_, Err(failure)) => Err(*failure),
            (Some(target), Ok(None)) => Ok(loader_module(*target)),
            (_, Ok(group)) => translate_factory(entry, &loaders, &aliases, group.as_ref()),
        };
        if let (Ok(_), Ok(Some(group))) = (&translation, group) {
            for facade in &group.foreign {
                facades.push((facade, group.primary, facade_filenames[&facade.id].clone()));
            }
        }
        match translation {
            Ok(Translation {
                params,
                body,
                residual,
            }) => {
                if let Some(letter) = residual {
                    notes.insert(
                        filename.clone(),
                        DetectedModuleNote::TurbopackRuntimeResidual(letter),
                    );
                }
                translated_index.push(Some(translated.len()));
                translated.push(TranslatedWebpackFactory {
                    id: id.clone(),
                    filename: filename.clone(),
                    params,
                    body,
                });
            }
            Err(failure) => {
                translated_index.push(None);
                failures.insert(filename.clone(), failure);
            }
        }
    }

    let facades: Vec<_> = facades
        .into_iter()
        .map(|(facade, primary, filename)| {
            let index = translated.len();
            let (params, body) = facade_module(primary, &facade.bindings);
            translated.push(TranslatedWebpackFactory {
                id: facade.id.to_string(),
                filename: filename.clone(),
                params,
                body,
            });
            (facade, filename, index)
        })
        .collect();

    let (mut prepared_translated, translated_failures) =
        prepare_translated_webpack_factories(&translated)?;
    failures.extend(translated_failures);
    if failures.len() == modules_entries.len() {
        return None;
    }
    notes.retain(|filename, _| !failures.contains_key(filename));

    let mut modules = Vec::with_capacity(modules_entries.len());
    let mut prepared: Vec<Option<PreparedModuleAst>> = Vec::with_capacity(modules_entries.len());
    for ((entry, id), (filename, index)) in modules_entries
        .iter()
        .zip(ids)
        .zip(filenames.into_iter().zip(translated_index))
    {
        let body = entry.body();
        modules.push(UnpackedModule {
            id,
            is_entry: false,
            code: source_fallback_for_stmts(&cm, body),
            filename,
            source_ranges: spans_byte_ranges(&cm, body.iter().map(|stmt| stmt.span())),
            ..Default::default()
        });
        prepared.push(index.and_then(|index| prepared_translated[index].take()));
    }
    for (facade, filename, index) in facades {
        // A facade has no source of its own; it points at the registrations
        // that defined its exports.
        modules.push(UnpackedModule {
            id: facade.id.to_string(),
            is_entry: false,
            filename,
            source_ranges: spans_byte_ranges(&cm, facade.spans.iter().copied()),
            ..Default::default()
        });
        prepared.push(prepared_translated[index].take());
    }
    if !prelude.is_empty() {
        // Top-level code beside the containers runs when the chunk loads,
        // outside every module. Each statement is copied separately because
        // the containers may sit between them.
        modules.push(UnpackedModule {
            id: "prelude".to_string(),
            is_entry: true,
            code: prelude
                .iter()
                .map(|stmt| source_fallback_for_stmts(&cm, std::slice::from_ref(*stmt)))
                .collect::<Vec<_>>()
                .join("\n"),
            filename: "prelude.js".to_string(),
            source_ranges: spans_byte_ranges(&cm, prelude.iter().map(|stmt| stmt.span())),
            ..Default::default()
        });
        prepared.push(None);
    }

    Some(
        DetectedBundle::new(
            // Chunks register into a runtime shared by every asset of the
            // app, so a module without a local importer is not dead.
            UnpackResult::new(modules, BundleFormat::Turbopack).with_external_consumers(),
            prepared,
            cm,
        )
        .with_module_failures(failures)
        .with_module_notes(notes),
    )
}

// ---------------------------------------------------------------------------
// Container shape
// ---------------------------------------------------------------------------

struct Entry<'a> {
    ids: Vec<usize>,
    factory: &'a Expr,
    /// From a 15.2–15.4 object container, which proves a runtime older than
    /// 16.1. Flat containers (15.5 and later) do not show 16.0 apart from
    /// 16.1.
    object_form: bool,
}

impl Entry<'_> {
    fn body(&self) -> &[Stmt] {
        factory_parts(self.factory)
            .map(|(_, body)| body)
            .unwrap_or_default()
    }
}

struct Chunk<'a> {
    entries: Vec<Entry<'a>>,
    /// Expression statements beside the containers, in source order.
    prelude: Vec<&'a Stmt>,
}

/// Collect every factory entry of a Turbopack chunk. Only expression
/// statements may appear beside the containers, which rules out a local
/// binding that shadows `globalThis` or `module`.
fn collect_entries<'a>(module: &'a Module, cm: &SourceMap) -> Option<Chunk<'a>> {
    let mut stmts = Vec::with_capacity(module.body.len());
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Empty(_)) => {}
            ModuleItem::Stmt(stmt @ Stmt::Expr(_)) if is_debug_id_polyfill(cm, stmt) => {}
            ModuleItem::Stmt(stmt @ Stmt::Expr(_)) => stmts.push(stmt),
            _ => return None,
        }
    }
    if let [Stmt::Expr(ExprStmt { expr, .. })] = stmts.as_slice() {
        if let Some(payload) = server_payload(expr) {
            return Some(Chunk {
                entries: server_entries(payload)?,
                prelude: Vec::new(),
            });
        }
    }
    let mut entries = Vec::new();
    let mut prelude = Vec::new();
    let mut saw_container = false;
    for stmt in stmts {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            unreachable!("only expression statements are collected");
        };
        match client_payload(expr) {
            Some(payload) => {
                saw_container = true;
                entries.extend(container_entries(&payload.elems[1..])?);
            }
            None => prelude.push(stmt),
        }
    }
    saw_container.then_some(Chunk { entries, prelude })
}

/// The polyfill that `turbopack.debugIds` (Next.js 16+) prepends to every
/// client and server chunk, around the chunk's debug id. It only records
/// that id in `globalThis._debugIds`, so it is dropped rather than emitted
/// as a prelude. The text is fixed in the Turbopack binary; anything else,
/// including a re-minified copy, stays in the prelude.
const DEBUG_ID_POLYFILL: (&str, &str) = (
    r#"!function(){try { var e="undefined"!=typeof globalThis?globalThis:"undefined"!=typeof global?global:"undefined"!=typeof window?window:"undefined"!=typeof self?self:{},n=(new e.Error).stack;n&&((e._debugIds|| (e._debugIds={}))[n]=""#,
    r#"")}catch(e){}}()"#,
);

fn is_debug_id_polyfill(cm: &SourceMap, stmt: &Stmt) -> bool {
    let text = source_fallback_for_stmts(cm, std::slice::from_ref(stmt));
    let text = text.strip_suffix(';').unwrap_or(&text);
    let (prefix, suffix) = DEBUG_ID_POLYFILL;
    text.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
        .is_some_and(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        })
}

fn server_entries(payload: &Expr) -> Option<Vec<Entry<'_>>> {
    let entries = match payload {
        Expr::Array(array) => payload_entries(&array.elems)?,
        Expr::Object(object) => object
            .props
            .iter()
            .map(object_entry)
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    entries.iter().any(uses_context_member).then_some(entries)
}

/// `(G || (G = [])).push([...])` with `G` naming one Turbopack global.
fn client_payload(expr: &Expr) -> Option<&ArrayLit> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "push") {
        return None;
    }
    if !is_turbopack_registry(&member.obj) {
        return None;
    }
    let [ExprOrSpread { spread: None, expr }] = call.args.as_slice() else {
        return None;
    };
    let Expr::Array(payload) = strip_parens(expr) else {
        return None;
    };
    // The first element is the script reference.
    payload.elems.first()?.as_ref()?;
    Some(payload)
}

/// `G || (G = [])` (15.5+) or `G = G || []` (15.3–15.4) for one global `G`.
fn is_turbopack_registry(expr: &Expr) -> bool {
    if let Some((global, right)) = logical_or_parts(expr) {
        return global_assignment(right)
            .is_some_and(|(target, value)| target == global && is_empty_array(value));
    }
    if let Some((target, value)) = global_assignment(expr) {
        return logical_or_parts(value)
            .is_some_and(|(global, right)| global == target && is_empty_array(right));
    }
    false
}

fn is_empty_array(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Array(array) if array.elems.is_empty())
}

/// `G = value` for a Turbopack global `G`.
fn global_assignment(expr: &Expr) -> Option<(String, &Expr)> {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assign.left else {
        return None;
    };
    Some((turbopack_global_member_name(target)?, &assign.right))
}

/// `G || right` for a Turbopack global `G`.
fn logical_or_parts(expr: &Expr) -> Option<(String, &Expr)> {
    let Expr::Bin(bin) = strip_parens(expr) else {
        return None;
    };
    if bin.op != swc_core::ecma::ast::BinaryOp::LogicalOr {
        return None;
    }
    Some((turbopack_global_name(&bin.left)?, &bin.right))
}

fn turbopack_global_name(expr: &Expr) -> Option<String> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    turbopack_global_member_name(member)
}

fn turbopack_global_member_name(member: &MemberExpr) -> Option<String> {
    let Expr::Ident(object) = strip_parens(&member.obj) else {
        return None;
    };
    if object.sym != "globalThis" {
        return None;
    }
    let name = match &member.prop {
        MemberProp::Ident(prop) => prop.sym.to_string(),
        MemberProp::Computed(ComputedPropName { expr, .. }) => match strip_parens(expr) {
            Expr::Lit(Lit::Str(name)) => name.value.as_str()?.to_string(),
            _ => return None,
        },
        MemberProp::PrivateName(_) => return None,
    };
    name.starts_with("TURBOPACK").then_some(name)
}

/// `module.exports = [...]` (15.5+) or `module.exports = {...}`, the server
/// chunk forms.
fn server_payload(expr: &Expr) -> Option<&Expr> {
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left: AssignTarget::Simple(SimpleAssignTarget::Member(target)),
        right,
        ..
    }) = strip_parens(expr)
    else {
        return None;
    };
    let Expr::Ident(object) = strip_parens(&target.obj) else {
        return None;
    };
    if object.sym != "module"
        || !matches!(&target.prop, MemberProp::Ident(prop) if prop.sym == "exports")
    {
        return None;
    }
    matches!(strip_parens(right), Expr::Array(_) | Expr::Object(_)).then(|| strip_parens(right))
}

/// Entries of a payload after its script element (client) or of the whole
/// payload (server). `Some(empty)` is a runtime registration such as
/// `[script, { otherChunks, runtimeModuleIds }]`, which holds no factories.
fn container_entries(elems: &[Option<ExprOrSpread>]) -> Option<Vec<Entry<'_>>> {
    let object = match elems.first() {
        Some(Some(ExprOrSpread { spread: None, expr })) => match strip_parens(expr) {
            Expr::Object(object) => object,
            _ => return payload_entries(elems),
        },
        _ => return payload_entries(elems),
    };
    let numeric_keys = object
        .props
        .iter()
        .filter(|prop| object_entry_id(prop).is_some())
        .count();
    if numeric_keys == 0 && !object.props.is_empty() {
        // Registration parameters, not a module object.
        return (elems.len() == 1).then(Vec::new);
    }
    if numeric_keys != object.props.len() {
        return None;
    }
    // 15.3 evaluate chunks append a registration parameter object.
    match &elems[1..] {
        [] => {}
        [Some(ExprOrSpread { spread: None, expr })] if matches!(strip_parens(expr), Expr::Object(params) if params.props.iter().all(|prop| object_entry_id(prop).is_none())) =>
            {}
        _ => return None,
    }
    object.props.iter().map(object_entry).collect()
}

fn object_entry_id(prop: &PropOrSpread) -> Option<usize> {
    let PropOrSpread::Prop(prop) = prop else {
        return None;
    };
    let Prop::KeyValue(KeyValueProp { key, .. }) = &**prop else {
        return None;
    };
    match key {
        PropName::Num(number) => numeric_id_from_expr(&Expr::Lit(Lit::Num(number.clone()))),
        PropName::Str(key) => numeric_id_string(key),
        _ => None,
    }
}

fn numeric_id_string(value: &Str) -> Option<usize> {
    let text = value.value.as_str()?;
    (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) && !text.starts_with("0")
        || text == "0")
        .then(|| text.parse().ok())
        .flatten()
}

/// `id: factory`, or 15.4's `id: [factory, [aliasId, ...]]`.
fn object_entry(prop: &PropOrSpread) -> Option<Entry<'_>> {
    let id = object_entry_id(prop)?;
    let PropOrSpread::Prop(prop) = prop else {
        return None;
    };
    let Prop::KeyValue(KeyValueProp { value, .. }) = &**prop else {
        return None;
    };
    if factory_parts(value).is_some() {
        return Some(Entry {
            ids: vec![id],
            factory: value,
            object_form: true,
        });
    }
    let Expr::Array(pair) = strip_parens(value) else {
        return None;
    };
    let [Some(ExprOrSpread {
        spread: None,
        expr: factory,
    }), Some(ExprOrSpread {
        spread: None,
        expr: aliases,
    })] = pair.elems.as_slice()
    else {
        return None;
    };
    factory_parts(factory)?;
    let Expr::Array(aliases) = strip_parens(aliases) else {
        return None;
    };
    let mut ids = vec![id];
    for alias in &aliases.elems {
        let ExprOrSpread { spread: None, expr } = alias.as_ref()? else {
            return None;
        };
        ids.push(match strip_parens(expr) {
            Expr::Lit(Lit::Str(alias)) => numeric_id_string(alias)?,
            expr => numeric_id_from_expr(expr)?,
        });
    }
    Some(Entry {
        ids,
        factory,
        object_form: true,
    })
}

/// Split a payload into id runs, each followed by one factory. Anything else,
/// including the strict-mode factory groups of unreleased Turbopack builds,
/// rejects the container.
fn payload_entries(elems: &[Option<ExprOrSpread>]) -> Option<Vec<Entry<'_>>> {
    let mut entries = Vec::new();
    let mut ids = Vec::new();
    for elem in elems {
        let ExprOrSpread { spread: None, expr } = elem.as_ref()? else {
            return None;
        };
        if let Some(id) = numeric_id_from_expr(expr) {
            ids.push(id);
            continue;
        }
        if ids.is_empty() || factory_parts(expr).is_none() {
            return None;
        }
        entries.push(Entry {
            ids: std::mem::take(&mut ids),
            factory: expr,
            object_form: false,
        });
    }
    if !ids.is_empty() || entries.is_empty() {
        return None;
    }
    Some(entries)
}

/// The factory's simple parameters and block body.
fn factory_parts(expr: &Expr) -> Option<(Vec<&Ident>, &[Stmt])> {
    let (params, body): (Vec<&Pat>, &[Stmt]) = match strip_parens(expr) {
        Expr::Arrow(ArrowExpr { params, body, .. }) => {
            let ArrowFunctionBody::FunctionBody(body) = &**body else {
                return None;
            };
            (params.iter().collect(), &body.stmts)
        }
        Expr::Fn(function) => {
            let function = &function.function;
            if function.is_async || function.is_generator {
                return None;
            }
            let body = function.body.as_ref()?;
            (
                function.params.iter().map(|param| &param.pat).collect(),
                &body.stmts,
            )
        }
        _ => return None,
    };
    if params.len() > 3 {
        return None;
    }
    let params = params
        .into_iter()
        .map(|pat| match pat {
            Pat::Ident(binding) => Some(&binding.id),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some((params, body))
}

/// Server chunks have no distinctive global, so `module.exports = [...]` is
/// accepted only when some factory calls a Turbopack module-protocol member
/// on its first parameter.
fn uses_context_member(entry: &Entry<'_>) -> bool {
    struct ContextMemberUse<'a> {
        ctx: &'a Atom,
        found: bool,
    }

    impl Visit for ContextMemberUse<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(member) = strip_parens(callee) {
                    if matches!(strip_parens(&member.obj), Expr::Ident(object) if object.sym == *self.ctx)
                        && matches!(&member.prop, MemberProp::Ident(prop) if matches!(prop.sym.as_ref(), "i" | "r" | "s" | "v" | "n"))
                    {
                        self.found = true;
                        return;
                    }
                }
            }
            call.visit_children_with(self);
        }
    }

    let Some((params, body)) = factory_parts(entry.factory) else {
        return false;
    };
    let Some(ctx) = params.first() else {
        return false;
    };
    let mut visitor = ContextMemberUse {
        ctx: &ctx.sym,
        found: false,
    };
    body.visit_with(&mut visitor);
    visitor.found
}

// ---------------------------------------------------------------------------
// Merged groups
// ---------------------------------------------------------------------------

/// A factory that defines several modules. Turbopack merges modules into
/// one factory, lists their ids before it, and registers each module's
/// exports with `ctx.s(bindings, id)`. The runtime runs the factory once,
/// for whichever id is required first, and each registration fills that
/// id's module-cache entry, so ids the registrations name without listing
/// them exist too. The factory becomes the primary module (the first listed
/// id), exporting every other member's bindings under aliases, and each
/// other member becomes a facade that re-exports them.
struct MergedGroup {
    primary: usize,
    foreign: Vec<Facade>,
}

struct Facade {
    id: usize,
    /// `(exported name, alias in the primary module)`, in registration order.
    bindings: Vec<(String, String)>,
    /// The registrations that define this member.
    spans: Vec<swc_core::common::Span>,
}

/// Each entry's merged group: `Ok(None)` when its registrations name no id
/// besides its first. A group whose shape is not proven fails closed.
fn merged_groups(
    entries: &[Entry<'_>],
    seen_ids: &HashSet<usize>,
) -> Vec<Result<Option<MergedGroup>, DetectedModuleFailure>> {
    let mut groups: Vec<_> = entries.iter().map(merged_group).collect();
    // An unlisted member must not be another factory's id, and two groups
    // must not both define one.
    let mut owners: HashMap<usize, Vec<usize>> = HashMap::default();
    for (index, (entry, group)) in entries.iter().zip(&groups).enumerate() {
        if let Ok(Some(group)) = group {
            for facade in &group.foreign {
                owners.entry(facade.id).or_default().push(index);
                if !entry.ids.contains(&facade.id) && seen_ids.contains(&facade.id) {
                    owners.entry(facade.id).or_default().push(usize::MAX);
                }
            }
        }
    }
    for indexes in owners.values().filter(|indexes| indexes.len() > 1) {
        for &index in indexes.iter().filter(|&&index| index != usize::MAX) {
            groups[index] = Err(DetectedModuleFailure::TurbopackUnsupportedRuntime(
                TurbopackContextUse::Member('s'),
            ));
        }
    }
    groups
}

fn merged_group(entry: &Entry<'_>) -> Result<Option<MergedGroup>, DetectedModuleFailure> {
    let unsupported = Err(DetectedModuleFailure::TurbopackUnsupportedRuntime(
        TurbopackContextUse::Member('s'),
    ));
    let Some((params, body)) = factory_parts(entry.factory) else {
        return Ok(None);
    };
    let Some(ctx) = params.first() else {
        return Ok(None);
    };
    let primary = entry.ids[0];
    // Top-level registrations, the only ones the translation accepts.
    let mut registrations: Vec<(Option<usize>, &Expr, swc_core::common::Span)> = Vec::new();
    for stmt in strip_directives(body) {
        let Stmt::Expr(ExprStmt { expr, span }) = stmt else {
            continue;
        };
        let elements: Vec<&Expr> = match strip_parens(expr) {
            Expr::Seq(seq) => seq.exprs.iter().map(|expr| &**expr).collect(),
            expr => vec![expr],
        };
        for element in elements {
            let Some(call) = ctx_member_call(element, ctx, "s") else {
                continue;
            };
            match call.args.as_slice() {
                [ExprOrSpread {
                    spread: None,
                    expr: bindings,
                }] => registrations.push((None, bindings, *span)),
                [ExprOrSpread {
                    spread: None,
                    expr: bindings,
                }, ExprOrSpread {
                    spread: None,
                    expr: id,
                }] => {
                    let Some(id) = numeric_id_from_expr(id) else {
                        return unsupported;
                    };
                    registrations.push((Some(id), bindings, *span));
                }
                _ => {}
            }
        }
    }
    if !registrations
        .iter()
        .any(|(id, ..)| id.is_some_and(|id| id != primary))
    {
        return Ok(None);
    }
    // A registration without an id would define whichever member was
    // required first.
    if registrations.iter().any(|(id, ..)| id.is_none()) {
        return unsupported;
    }
    // A listed member without registrations would be an empty module.
    if !entry
        .ids
        .iter()
        .all(|listed| registrations.iter().any(|(id, ..)| *id == Some(*listed)))
    {
        return unsupported;
    }

    let mut taken: HashSet<String> = HashSet::default();
    for (_, bindings, _) in registrations.iter().filter(|(id, ..)| *id == Some(primary)) {
        let Some(names) = esm_binding_names(bindings) else {
            return unsupported;
        };
        taken.extend(names);
    }
    let mut foreign: Vec<Facade> = Vec::new();
    for (id, bindings, span) in &registrations {
        let id = id.expect("registrations without an id were rejected");
        if id == primary {
            continue;
        }
        let Some(names) = esm_binding_names(bindings) else {
            return unsupported;
        };
        let index = match foreign.iter().position(|facade| facade.id == id) {
            Some(index) => index,
            None => {
                foreign.push(Facade {
                    id,
                    bindings: Vec::new(),
                    spans: Vec::new(),
                });
                foreign.len() - 1
            }
        };
        let facade = &mut foreign[index];
        facade.spans.push(*span);
        for name in names {
            // The runtime keeps the first definition of a name.
            if facade
                .bindings
                .iter()
                .any(|(exported, _)| *exported == name)
            {
                return unsupported;
            }
            let mut alias = if name == "default" || taken.contains(&name) {
                format!("{name}_{id}")
            } else {
                name.clone()
            };
            let mut suffix = 1;
            while taken.contains(&alias) {
                suffix += 1;
                alias = format!("{name}_{id}_{suffix}");
            }
            taken.insert(alias.clone());
            facade.bindings.push((name, alias));
        }
    }
    Ok(Some(MergedGroup { primary, foreign }))
}

/// The exported names of a `ctx.s` binding list, in order, under the same
/// shapes the translation accepts.
fn esm_binding_names(bindings: &Expr) -> Option<Vec<String>> {
    let mut names = Vec::new();
    match strip_parens(bindings) {
        Expr::Object(object) => {
            for prop in &object.props {
                let PropOrSpread::Prop(prop) = prop else {
                    return None;
                };
                let Prop::KeyValue(KeyValueProp { key, .. }) = &**prop else {
                    return None;
                };
                names.push(match key {
                    PropName::Ident(key) => key.sym.to_string(),
                    PropName::Str(key) => key.value.as_str()?.to_string(),
                    _ => return None,
                });
            }
        }
        Expr::Array(array) => {
            let mut elements = array.elems.iter().map(|elem| match elem {
                Some(ExprOrSpread { spread: None, expr }) => Some(&**expr),
                _ => None,
            });
            while let Some(name) = elements.next() {
                let Expr::Lit(Lit::Str(name)) = name? else {
                    return None;
                };
                names.push(name.value.as_str()?.to_string());
                let next = elements.next()??;
                if matches!(next, Expr::Lit(Lit::Num(Number { value, .. })) if *value == 0.0) {
                    elements.next()??;
                }
            }
        }
        _ => return None,
    }
    Some(names)
}

/// A member of a merged group: re-export its aliased bindings from the
/// primary module through `Object.defineProperty` getters, the CommonJS
/// shape that ESM recovery turns into a live `export { alias as name } from`.
fn facade_module(primary: usize, bindings: &[(String, String)]) -> (Vec<Pat>, Vec<Stmt>) {
    let [module, exports, require, target] =
        ["module", "exports", "require", "primary"].map(Atom::from);
    let mut body = vec![
        var_stmt(
            Ident::new_no_ctxt(target.clone(), Default::default()),
            call_expr(ident_expr(&require), vec![number_expr(primary)]),
        ),
        expr_stmt(call_expr(
            member(ident_expr(&require), "r"),
            vec![ident_expr(&exports)],
        )),
    ];
    for (name, alias) in bindings {
        let descriptor = ObjectLit {
            span: Default::default(),
            props: vec![
                PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                    key: prop_name("enumerable"),
                    value: Box::new(Expr::Lit(Lit::Bool(true.into()))),
                }))),
                PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                    key: prop_name("get"),
                    value: Box::new(Expr::Arrow(ArrowExpr {
                        body: Box::new(ArrowFunctionBody::Expr(member(ident_expr(&target), alias))),
                        ..Default::default()
                    })),
                }))),
            ],
        };
        body.push(expr_stmt(call_expr(
            member(ident_expr(&Atom::from("Object")), "defineProperty"),
            vec![
                ident_expr(&exports),
                Box::new(Expr::Lit(Lit::Str(Str {
                    span: Default::default(),
                    value: name.as_str().into(),
                    raw: None,
                }))),
                Box::new(Expr::Object(descriptor)),
            ],
        )));
    }
    let params = [module, exports, require]
        .into_iter()
        .map(|sym| {
            Pat::Ident(BindingIdent {
                id: Ident::new_no_ctxt(sym, Default::default()),
                type_ann: None,
            })
        })
        .collect();
    (params, body)
}

// ---------------------------------------------------------------------------
// Async loader modules
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum LoaderTarget {
    Module(usize),
    /// The loader only loads chunks and resolves to nothing.
    Empty,
}

/// Match the async loader module Turbopack generates for `import()`:
/// `ctx.v((parentImport) => <load chunks>.then(() => parentImport(id)))`,
/// with an empty, `Promise.resolve()`, or `Promise.all(chunks.map(ctx.l))`
/// chunk step.
fn loader_target(factory: &Expr) -> Option<LoaderTarget> {
    let (params, body) = factory_parts(factory)?;
    let ctx = *params.first()?;
    let [statement] = strip_context_preamble(strip_directives(body), ctx) else {
        return None;
    };
    let Stmt::Expr(ExprStmt { expr, .. }) = statement else {
        return None;
    };
    let call = ctx_member_call(expr, ctx, "v")?;
    let [ExprOrSpread {
        spread: None,
        expr: loader,
    }] = call.args.as_slice()
    else {
        return None;
    };
    let (loader_params, loader_body) = function_returning(loader)?;
    let [parent_import] = loader_params.as_slice() else {
        return None;
    };
    if parent_import.sym == ctx.sym {
        return None;
    }

    if is_promise_resolve(loader_body) {
        return Some(LoaderTarget::Empty);
    }
    let Expr::Call(then_call) = strip_parens(loader_body) else {
        return None;
    };
    let Callee::Expr(then_callee) = &then_call.callee else {
        return None;
    };
    let Expr::Member(then_member) = strip_parens(then_callee) else {
        return None;
    };
    if !matches!(&then_member.prop, MemberProp::Ident(prop) if prop.sym == "then") {
        return None;
    }
    if !is_promise_resolve(&then_member.obj) && !is_chunk_load(&then_member.obj, ctx) {
        return None;
    }
    let [ExprOrSpread {
        spread: None,
        expr: callback,
    }] = then_call.args.as_slice()
    else {
        return None;
    };
    let (callback_params, callback_body) = function_returning_or_empty(callback)?;
    if !callback_params.is_empty() {
        return None;
    }
    let Some(callback_body) = callback_body else {
        return Some(LoaderTarget::Empty);
    };
    let Expr::Call(import_call) = strip_parens(callback_body) else {
        return None;
    };
    let Callee::Expr(import_callee) = &import_call.callee else {
        return None;
    };
    if !matches!(strip_parens(import_callee), Expr::Ident(callee) if callee.sym == parent_import.sym)
    {
        return None;
    }
    let [ExprOrSpread {
        spread: None,
        expr: id,
    }] = import_call.args.as_slice()
    else {
        return None;
    };
    Some(LoaderTarget::Module(numeric_id_from_expr(id)?))
}

fn strip_directives(body: &[Stmt]) -> &[Stmt] {
    let start = body
        .iter()
        .take_while(|stmt| {
            matches!(stmt, Stmt::Expr(ExprStmt { expr, .. }) if matches!(&**expr, Expr::Lit(Lit::Str(_))))
        })
        .count();
    &body[start..]
}

/// Skip a 15.3–15.4 `var { g, __dirname, ... } = ctx;` preamble.
fn strip_context_preamble<'a>(body: &'a [Stmt], ctx: &Ident) -> &'a [Stmt] {
    match body.split_first() {
        Some((first, rest))
            if matches!(first, Stmt::Decl(swc_core::ecma::ast::Decl::Var(var)) if var.decls.len() == 1)
                && context_preamble(first, &ctx.sym).is_some() =>
        {
            rest
        }
        _ => body,
    }
}

/// A `var { key: binding, ... } = ctx;` preamble: the declarator index and
/// its `(key, binding)` pairs. A minifier may merge other declarators into
/// the same `var`.
fn context_preamble<'a>(stmt: &'a Stmt, ctx: &Atom) -> Option<(usize, Vec<(Atom, &'a Ident)>)> {
    let Stmt::Decl(swc_core::ecma::ast::Decl::Var(var)) = stmt else {
        return None;
    };
    if var.kind != swc_core::ecma::ast::VarDeclKind::Var {
        return None;
    }
    let (index, pattern) = var.decls.iter().enumerate().find_map(|(index, declarator)| {
        let Pat::Object(pattern) = &declarator.name else {
            return None;
        };
        matches!(declarator.init.as_deref().map(strip_parens), Some(Expr::Ident(init)) if init.sym == *ctx)
            .then_some((index, pattern))
    })?;
    let bindings = pattern
        .props
        .iter()
        .map(|prop| match prop {
            swc_core::ecma::ast::ObjectPatProp::KeyValue(prop) => {
                let key = match &prop.key {
                    PropName::Ident(key) => key.sym.clone(),
                    _ => return None,
                };
                match &*prop.value {
                    Pat::Ident(binding) => Some((key, &binding.id)),
                    _ => None,
                }
            }
            swc_core::ecma::ast::ObjectPatProp::Assign(prop) if prop.value.is_none() => {
                Some((prop.key.id.sym.clone(), &prop.key.id))
            }
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some((index, bindings))
}

/// A function whose body is one returned expression.
fn function_returning(expr: &Expr) -> Option<(Vec<&Ident>, &Expr)> {
    let (params, body) = function_returning_or_empty(expr)?;
    Some((params, body?))
}

/// Like [`function_returning`], also accepting an empty block body (`None`).
fn function_returning_or_empty(expr: &Expr) -> Option<(Vec<&Ident>, Option<&Expr>)> {
    match strip_parens(expr) {
        Expr::Arrow(arrow) if !arrow.is_async && !arrow.is_generator => {
            let params = simple_params(&arrow.params)?;
            match &*arrow.body {
                ArrowFunctionBody::Expr(body) => Some((params, Some(&**body))),
                ArrowFunctionBody::FunctionBody(block) => {
                    Some((params, block_return(&block.stmts)?))
                }
            }
        }
        Expr::Fn(function) if !function.function.is_async && !function.function.is_generator => {
            let function = &function.function;
            let params = simple_params(function.params.iter().map(|param| &param.pat))?;
            Some((params, block_return(&function.body.as_ref()?.stmts)?))
        }
        _ => None,
    }
}

fn simple_params<'a>(pats: impl IntoIterator<Item = &'a Pat>) -> Option<Vec<&'a Ident>> {
    pats.into_iter()
        .map(|pat| match pat {
            Pat::Ident(binding) => Some(&binding.id),
            _ => None,
        })
        .collect()
}

fn block_return(stmts: &[Stmt]) -> Option<Option<&Expr>> {
    match stmts {
        [] => Some(None),
        [Stmt::Return(ReturnStmt { arg: Some(arg), .. })] => Some(Some(&**arg)),
        _ => None,
    }
}

fn is_promise_resolve(expr: &Expr) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    if !call.args.is_empty() {
        return false;
    }
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    is_promise_member(callee, "resolve")
}

fn is_promise_member(expr: &Expr, name: &str) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    matches!(strip_parens(&member.obj), Expr::Ident(object) if object.sym == "Promise")
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == name)
}

/// `Promise.all([chunks...].map((chunk) => ctx.l(chunk)))`.
fn is_chunk_load(expr: &Expr, ctx: &Ident) -> bool {
    let Expr::Call(all) = strip_parens(expr) else {
        return false;
    };
    let Callee::Expr(all_callee) = &all.callee else {
        return false;
    };
    if !is_promise_member(all_callee, "all") {
        return false;
    }
    let [ExprOrSpread {
        spread: None,
        expr: mapped,
    }] = all.args.as_slice()
    else {
        return false;
    };
    let Expr::Call(map) = strip_parens(mapped) else {
        return false;
    };
    let Callee::Expr(map_callee) = &map.callee else {
        return false;
    };
    let Expr::Member(map_member) = strip_parens(map_callee) else {
        return false;
    };
    if !matches!(&map_member.prop, MemberProp::Ident(prop) if prop.sym == "map") {
        return false;
    }
    let Expr::Array(chunks) = strip_parens(&map_member.obj) else {
        return false;
    };
    if chunks.elems.iter().any(|elem| {
        !matches!(elem, Some(ExprOrSpread { spread: None, expr }) if matches!(strip_parens(expr), Expr::Lit(Lit::Str(_)) | Expr::Object(_)))
    }) {
        return false;
    }
    let [ExprOrSpread {
        spread: None,
        expr: callback,
    }] = map.args.as_slice()
    else {
        return false;
    };
    let Some((params, Some(body))) = function_returning_or_empty(callback) else {
        return false;
    };
    let [chunk] = params.as_slice() else {
        return false;
    };
    if chunk.sym == ctx.sym {
        return false;
    }
    let Some(load) = ctx_member_call(body, ctx, "l") else {
        return false;
    };
    matches!(load.args.as_slice(), [ExprOrSpread { spread: None, expr }] if matches!(strip_parens(expr), Expr::Ident(arg) if arg.sym == chunk.sym))
}

/// `ctx.<name>(...)`, matched by spelling. Only used on the closed loader
/// shape, whose inner parameters are checked not to shadow `ctx`.
fn ctx_member_call<'a>(expr: &'a Expr, ctx: &Ident, name: &str) -> Option<&'a CallExpr> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    let Expr::Ident(object) = strip_parens(&member.obj) else {
        return None;
    };
    (object.sym == ctx.sym && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == name))
        .then_some(call)
}

// ---------------------------------------------------------------------------
// Factory translation
// ---------------------------------------------------------------------------

/// The free name that runtime residuals keep calling through. It is
/// undefined once modules are split, which the residual diagnostic reports.
const RESIDUAL_CONTEXT: &str = "__turbopack_context__";

/// A factory in webpack's `(module, exports, require)` form.
struct Translation {
    params: Vec<Pat>,
    body: Vec<Stmt>,
    /// The first runtime member kept as a call through
    /// [`RESIDUAL_CONTEXT`].
    residual: Option<char>,
}

/// Translate one factory into webpack's `(module, exports, require)` form.
/// An error means the factory uses its context in a way without a known
/// translation and must stay opaque.
fn translate_factory(
    entry: &Entry<'_>,
    loaders: &HashMap<usize, LoaderTarget>,
    aliases: &HashMap<usize, usize>,
    group: Option<&MergedGroup>,
) -> Result<Translation, DetectedModuleFailure> {
    let (params, body) =
        factory_parts(entry.factory).expect("payload entries hold validated factories");
    let globals = Globals::new();
    GLOBALS.set(&globals, || {
        let mut module = Module {
            span: Default::default(),
            body: body.iter().cloned().map(ModuleItem::Stmt).collect(),
            shebang: None,
        };
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        let Some(ctx) = params.first() else {
            // A factory without parameters uses no runtime helper.
            return Ok(Translation {
                params: Vec::new(),
                body: clear_contexts(module),
                residual: None,
            });
        };
        let mut names = AllNames::default();
        module.visit_with(&mut names);
        let preamble = lower_context_preamble(&mut module, &ctx.sym, params.len(), &mut names)?;
        // The webpack normalizer renames this parameter to `require` anyway;
        // a fresh spelling keeps a block that redeclares the original name
        // from capturing context references once blocks are flattened.
        // The same holds for the module and exports parameters, and fresh
        // spellings also keep synthesized references to them uncaptured.
        let ctx_name = names.fresh("context");
        let module_name = preamble.module.unwrap_or_else(|| names.fresh("module"));
        let exports_name = preamble.exports.unwrap_or_else(|| names.fresh("exports"));
        let context_helper = names.fresh("moduleContext");
        let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
        let renames = params
            .iter()
            .zip([&ctx_name, &module_name, &exports_name])
            .map(|(param, fresh)| BindingRename {
                old: (param.sym.clone(), unresolved_ctxt),
                new: fresh.clone(),
            })
            .collect::<Vec<_>>();
        rename_bindings_in_module(&mut module, &renames);
        flatten_top_level_blocks(&mut module);

        let mut translator = ContextTranslator {
            ctx: ctx_name.clone(),
            unresolved_mark,
            module_name: module_name.clone(),
            exports_name: exports_name.clone(),
            own_ids: &entry.ids,
            group,
            loaders,
            aliases,
            failure: None,
            uses_global_this: preamble.uses_global_this,
            uses_promise: false,
            context_helper: context_helper.clone(),
            uses_context_helper: false,
            residual: None,
        };
        translator.translate_module(&mut module);
        if let Some(failure) = translator.failure {
            return Err(failure);
        }
        // A synthesized global reference must not be captured by a local
        // binding of the same spelling.
        if translator.uses_global_this && names.bindings.contains(&Atom::from("globalThis")) {
            return Err(DetectedModuleFailure::TurbopackUnsupportedRuntime(
                TurbopackContextUse::Member('g'),
            ));
        }
        if translator.uses_promise && names.bindings.contains(&Atom::from("Promise")) {
            return Err(DetectedModuleFailure::TurbopackUnsupportedRuntime(
                TurbopackContextUse::Member('A'),
            ));
        }
        if translator.uses_context_helper {
            if ["Object", "Error"]
                .iter()
                .any(|global| names.bindings.contains(&Atom::from(*global)))
            {
                return Err(DetectedModuleFailure::TurbopackUnsupportedRuntime(
                    TurbopackContextUse::Member('f'),
                ));
            }
            let directives = module
                .body
                .iter()
                .take_while(|item| {
                    matches!(item, ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) if matches!(&**expr, Expr::Lit(Lit::Str(_))))
                })
                .count();
            module.body.insert(
                directives,
                ModuleItem::Stmt(module_context_helper(
                    &context_helper,
                    !entry.object_form,
                )),
            );
        }
        // The residual must stay free in the emitted module.
        if let Some(letter) = translator.residual {
            if names.all.contains(&Atom::from(RESIDUAL_CONTEXT)) {
                return Err(DetectedModuleFailure::TurbopackUnsupportedRuntime(
                    TurbopackContextUse::Member(letter),
                ));
            }
        }

        let params = [module_name, exports_name, ctx_name]
            .into_iter()
            .map(|sym| {
                Pat::Ident(BindingIdent {
                    id: Ident::new_no_ctxt(sym, Default::default()),
                    type_ann: None,
                })
            })
            .collect();
        Ok(Translation {
            params,
            body: clear_contexts(module),
            residual: translator.residual,
        })
    })
}

/// A generated async loader module, translated to export a function that
/// loads its target. Chunk loading has no meaning once modules are split.
fn loader_module(target: LoaderTarget) -> Translation {
    let [module, exports, require] = ["module", "exports", "require"].map(Atom::from);
    let load = match target {
        LoaderTarget::Module(id) => {
            let require_target = call_expr(ident_expr(&require), vec![number_expr(id)]);
            let deferred = Box::new(Expr::Arrow(ArrowExpr {
                body: Box::new(ArrowFunctionBody::Expr(require_target)),
                ..Default::default()
            }));
            call_expr(member(promise_resolve(), "then"), vec![deferred])
        }
        LoaderTarget::Empty => promise_resolve(),
    };
    let loader = Box::new(Expr::Arrow(ArrowExpr {
        body: Box::new(ArrowFunctionBody::Expr(load)),
        ..Default::default()
    }));
    let params = [module.clone(), exports, require]
        .into_iter()
        .map(|sym| {
            Pat::Ident(BindingIdent {
                id: Ident::new_no_ctxt(sym, Default::default()),
                type_ann: None,
            })
        })
        .collect();
    Translation {
        params,
        body: vec![assign_stmt(member(ident_expr(&module), "exports"), loader)],
        residual: None,
    }
}

fn number_expr(value: usize) -> Box<Expr> {
    Box::new(Expr::Lit(Lit::Num(Number {
        span: Default::default(),
        value: value as f64,
        raw: None,
    })))
}

/// What a 15.3–15.4 context preamble bound.
#[derive(Default)]
struct ContextPreamble {
    module: Option<Atom>,
    exports: Option<Atom>,
    uses_global_this: bool,
}

/// Remove a `var { g, __dirname, m, e } = ctx;` preamble. References to the
/// `m`/`e` bindings are renamed to fresh module and exports parameter names
/// (a later block may redeclare the original spelling); a used `g` becomes
/// `var x = globalThis`. Any other used binding has no translation.
fn lower_context_preamble(
    module: &mut Module,
    ctx: &Atom,
    param_count: usize,
    names: &mut AllNames,
) -> Result<ContextPreamble, DetectedModuleFailure> {
    let unsupported = DetectedModuleFailure::TurbopackUnsupportedRuntime;
    let index = module
        .body
        .iter()
        .take_while(|item| {
            matches!(item, ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) if matches!(&**expr, Expr::Lit(Lit::Str(_))))
        })
        .count();
    let Some(ModuleItem::Stmt(stmt)) = module.body.get(index) else {
        return Ok(ContextPreamble::default());
    };
    let Some((declarator, bindings)) = context_preamble(stmt, ctx) else {
        return Ok(ContextPreamble::default());
    };
    let bindings: Vec<(Atom, Ident)> = bindings
        .into_iter()
        .map(|(key, binding)| (key, binding.clone()))
        .collect();
    let mut references = BindingReferences::default();
    module.visit_with(&mut references);

    let mut preamble = ContextPreamble::default();
    let mut replacement = Vec::new();
    let mut renames = Vec::new();
    for (key, binding) in bindings {
        // The declaration itself is one occurrence.
        let used = references.count(&binding) > 1;
        match key.as_ref() {
            "m" | "e" if param_count > 1 => return Err(unsupported(context_use(&key))),
            "m" | "e" => {
                let fresh = names.fresh(if key == "m" { "module" } else { "exports" });
                renames.push(BindingRename {
                    old: (binding.sym.clone(), binding.ctxt),
                    new: fresh.clone(),
                });
                if key == "m" {
                    preamble.module = Some(fresh);
                } else {
                    preamble.exports = Some(fresh);
                }
            }
            "g" if used => {
                preamble.uses_global_this = true;
                replacement.push(ModuleItem::Stmt(var_stmt(
                    binding.clone(),
                    ident_expr(&Atom::from("globalThis")),
                )));
            }
            _ if used => return Err(unsupported(context_use(&key))),
            _ => {}
        }
    }
    let ModuleItem::Stmt(Stmt::Decl(swc_core::ecma::ast::Decl::Var(var))) = &mut module.body[index]
    else {
        unreachable!("context_preamble matched a var declaration");
    };
    var.decls.remove(declarator);
    let keep = usize::from(!var.decls.is_empty());
    module.body.splice(index + keep..=index, replacement);
    rename_bindings_in_module(module, &renames);
    Ok(preamble)
}

fn context_use(name: &str) -> TurbopackContextUse {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        _ if name == "__dirname" => TurbopackContextUse::Dirname,
        (Some(letter), None) => TurbopackContextUse::Member(letter),
        _ => TurbopackContextUse::Other,
    }
}

/// Resolved identifier occurrences, counted per binding.
#[derive(Default)]
struct BindingReferences {
    counts: HashMap<(Atom, SyntaxContext), usize>,
}

impl BindingReferences {
    fn count(&self, ident: &Ident) -> usize {
        self.counts
            .get(&(ident.sym.clone(), ident.ctxt))
            .copied()
            .unwrap_or_default()
    }
}

impl Visit for BindingReferences {
    fn visit_ident(&mut self, ident: &Ident) {
        *self
            .counts
            .entry((ident.sym.clone(), ident.ctxt))
            .or_default() += 1;
    }
}

/// 15.3–15.4 wrap a factory body in a block after the preamble. Splice such
/// blocks into the top level when their own declarations cannot collide
/// with any other top-level name, so export registrations become top-level
/// statements. Leading string directives inside the block are dropped.
fn flatten_top_level_blocks(module: &mut Module) {
    if !module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::Stmt(Stmt::Block(_))))
    {
        return;
    }
    let block_names = |block: &swc_core::ecma::ast::BlockStmt| -> Vec<Atom> {
        block
            .stmts
            .iter()
            .flat_map(|stmt| super::module_item_declared_names(&ModuleItem::Stmt(stmt.clone())))
            .collect()
    };
    let mut body = Vec::with_capacity(module.body.len());
    for (index, item) in module.body.iter().enumerate() {
        let ModuleItem::Stmt(Stmt::Block(block)) = item else {
            body.push(item.clone());
            continue;
        };
        let own = block_names(block);
        let collides = module.body.iter().enumerate().any(|(other, item)| {
            other != index
                && match item {
                    ModuleItem::Stmt(Stmt::Block(other)) => block_names(other),
                    item => super::module_item_declared_names(item),
                }
                .iter()
                .any(|name| own.contains(name))
        });
        if collides {
            body.push(item.clone());
            continue;
        }
        let directives = block
            .stmts
            .iter()
            .take_while(|stmt| {
                matches!(stmt, Stmt::Expr(ExprStmt { expr, .. }) if matches!(&**expr, Expr::Lit(Lit::Str(_))))
            })
            .count();
        body.extend(
            block.stmts[directives..]
                .iter()
                .cloned()
                .map(ModuleItem::Stmt),
        );
    }
    module.body = body;
}

fn var_stmt(binding: Ident, init: Box<Expr>) -> Stmt {
    Stmt::Decl(swc_core::ecma::ast::Decl::Var(Box::new(
        swc_core::ecma::ast::VarDecl {
            kind: swc_core::ecma::ast::VarDeclKind::Var,
            decls: vec![swc_core::ecma::ast::VarDeclarator {
                span: Default::default(),
                name: Pat::Ident(BindingIdent {
                    id: binding,
                    type_ann: None,
                }),
                init: Some(init),
                definite: false,
            }],
            ..Default::default()
        },
    )))
}

fn clear_contexts(mut module: Module) -> Vec<Stmt> {
    struct ClearContexts;

    impl VisitMut for ClearContexts {
        fn visit_mut_syntax_context(&mut self, ctxt: &mut SyntaxContext) {
            *ctxt = SyntaxContext::empty();
        }
    }

    module.visit_mut_with(&mut ClearContexts);
    module
        .body
        .into_iter()
        .filter_map(|item| match item {
            ModuleItem::Stmt(stmt) => Some(stmt),
            ModuleItem::ModuleDecl(_) => None,
        })
        .collect()
}

/// Every identifier spelling in the factory, and the spellings bound by a
/// declaration anywhere inside it.
#[derive(Default)]
struct AllNames {
    all: HashSet<Atom>,
    bindings: HashSet<Atom>,
}

impl AllNames {
    fn fresh(&mut self, base: &str) -> Atom {
        let mut candidate = Atom::from(base);
        let mut suffix = 1;
        while self.all.contains(&candidate) {
            suffix += 1;
            candidate = Atom::from(format!("{base}_{suffix}"));
        }
        self.all.insert(candidate.clone());
        candidate
    }
}

impl Visit for AllNames {
    fn visit_ident(&mut self, ident: &Ident) {
        self.all.insert(ident.sym.clone());
    }

    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        self.bindings.insert(binding.id.sym.clone());
        binding.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.bindings.insert(decl.ident.sym.clone());
        decl.visit_children_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        self.bindings.insert(decl.ident.sym.clone());
        decl.visit_children_with(self);
    }

    fn visit_fn_expr(&mut self, expr: &swc_core::ecma::ast::FnExpr) {
        if let Some(ident) = &expr.ident {
            self.bindings.insert(ident.sym.clone());
        }
        expr.visit_children_with(self);
    }

    fn visit_class_expr(&mut self, expr: &swc_core::ecma::ast::ClassExpr) {
        if let Some(ident) = &expr.ident {
            self.bindings.insert(ident.sym.clone());
        }
        expr.visit_children_with(self);
    }
}

struct ContextTranslator<'a> {
    ctx: Atom,
    unresolved_mark: Mark,
    module_name: Atom,
    exports_name: Atom,
    own_ids: &'a [usize],
    group: Option<&'a MergedGroup>,
    loaders: &'a HashMap<usize, LoaderTarget>,
    aliases: &'a HashMap<usize, usize>,
    failure: Option<DetectedModuleFailure>,
    uses_global_this: bool,
    uses_promise: bool,
    /// The local copy of the runtime's `require.context` implementation
    /// that `ctx.f` translates to, and whether the factory needs it.
    context_helper: Atom,
    uses_context_helper: bool,
    /// The first runtime member kept as a residual.
    residual: Option<char>,
}

/// Top-level export registration, translated at statement level.
enum ExportCall {
    Esm,
    Value,
}

/// The module an export registration defines.
enum ExportTarget {
    Own,
    /// Another member of the merged group, by id.
    Foreign(usize),
}

impl ContextTranslator<'_> {
    /// Record the first context use without a translation. `member` is the
    /// runtime letter, or `None` when the context escapes as a value.
    fn reject(&mut self, member: Option<&str>) {
        if self.failure.is_none() {
            let context_use = member.map_or(TurbopackContextUse::Other, context_use);
            self.failure = Some(DetectedModuleFailure::TurbopackUnsupportedRuntime(
                context_use,
            ));
        }
    }

    fn is_ctx(&self, expr: &Expr) -> bool {
        matches!(strip_parens(expr), Expr::Ident(ident) if ident.sym == self.ctx && ident.ctxt.outer() == self.unresolved_mark)
    }

    /// `ctx.<letter>` with a free `ctx`.
    fn ctx_member<'e>(&self, expr: &'e Expr) -> Option<&'e str> {
        let Expr::Member(member) = strip_parens(expr) else {
            return None;
        };
        if !self.is_ctx(&member.obj) {
            return None;
        }
        match &member.prop {
            MemberProp::Ident(prop) => Some(prop.sym.as_ref()),
            _ => None,
        }
    }

    fn export_call(&self, expr: &Expr) -> Option<ExportCall> {
        let Expr::Call(call) = strip_parens(expr) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        match self.ctx_member(callee)? {
            "s" => Some(ExportCall::Esm),
            "v" | "n" | "q" => Some(ExportCall::Value),
            _ => None,
        }
    }

    fn translate_module(&mut self, module: &mut Module) {
        let mut body = Vec::with_capacity(module.body.len());
        for item in std::mem::take(&mut module.body) {
            let ModuleItem::Stmt(Stmt::Expr(ExprStmt { span, expr })) = item else {
                let mut item = item;
                item.visit_mut_with(self);
                body.push(item);
                continue;
            };
            let elements: Vec<Box<Expr>> = match *expr {
                Expr::Seq(seq)
                    if seq
                        .exprs
                        .iter()
                        .any(|expr| self.export_call(expr).is_some()) =>
                {
                    seq.exprs
                }
                expr => vec![Box::new(expr)],
            };
            for mut element in elements {
                match self.export_call(&element) {
                    Some(kind) => {
                        let Expr::Call(call) = *element else {
                            unreachable!("export_call matched a call");
                        };
                        let letter = match &call.callee {
                            Callee::Expr(callee) => self.ctx_member(callee).map(str::to_owned),
                            _ => None,
                        };
                        let Some(stmts) = self.translate_export(call, kind) else {
                            self.reject(letter.as_deref());
                            return;
                        };
                        body.extend(stmts.into_iter().map(ModuleItem::Stmt));
                    }
                    None if self.is_require_read(&element) => {}
                    None => {
                        self.visit_discarded(&mut element);
                        body.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                            span,
                            expr: element,
                        })));
                    }
                }
            }
        }
        module.body = body;
    }

    /// `ctx.r` read without a call. The member is a plain method, so a read
    /// whose value is discarded has no effect.
    fn is_require_read(&self, expr: &Expr) -> bool {
        self.ctx_member(expr) == Some("r")
    }

    /// Visit an expression whose value is discarded. A value export there
    /// (`ctx.v(x)`, `ctx.n(x)`, `ctx.q(url)`) returns `undefined`, so an
    /// assignment to `module.exports` is equivalent.
    fn visit_discarded(&mut self, expr: &mut Expr) {
        if self.failure.is_some() {
            return;
        }
        match expr {
            Expr::Call(_) if matches!(self.export_call(expr), Some(ExportCall::Value)) => {
                let Expr::Call(call) = std::mem::take(expr) else {
                    unreachable!("export_call matched a call");
                };
                let letter = match &call.callee {
                    Callee::Expr(callee) => self.ctx_member(callee).map(str::to_owned),
                    _ => None,
                };
                match self.value_export(call) {
                    Some(assignment) => *expr = *assignment,
                    None => self.reject(letter.as_deref()),
                }
            }
            // An arrow IIFE evaluates to its expression body, so discarding
            // the call discards the body too (the AMD `define` wrapper).
            Expr::Call(call) if is_arrow_iife(call) => {
                call.args.visit_mut_with(self);
                let Callee::Expr(callee) = &mut call.callee else {
                    unreachable!("is_arrow_iife matched an expression callee");
                };
                let Expr::Arrow(arrow) = strip_parens_mut(callee) else {
                    unreachable!("is_arrow_iife matched an arrow");
                };
                arrow.params.visit_mut_with(self);
                let ArrowFunctionBody::Expr(body) = &mut *arrow.body else {
                    unreachable!("is_arrow_iife matched an expression body");
                };
                self.visit_discarded(body);
            }
            Expr::Seq(seq) => {
                // The AMD branch of a UMD wrapper reads `ctx.r` for the
                // `define` dependency it never uses.
                let last = seq.exprs.len() - 1;
                let mut index = 0;
                seq.exprs.retain(|element| {
                    index += 1;
                    index - 1 == last || !self.is_require_read(element)
                });
                for element in &mut seq.exprs {
                    self.visit_discarded(element);
                }
            }
            Expr::Bin(bin)
                if matches!(
                    bin.op,
                    swc_core::ecma::ast::BinaryOp::LogicalAnd
                        | swc_core::ecma::ast::BinaryOp::LogicalOr
                        | swc_core::ecma::ast::BinaryOp::NullishCoalescing
                ) =>
            {
                bin.left.visit_mut_with(self);
                self.visit_discarded(&mut bin.right);
            }
            Expr::Cond(cond) => {
                cond.test.visit_mut_with(self);
                self.visit_discarded(&mut cond.cons);
                self.visit_discarded(&mut cond.alt);
            }
            Expr::Paren(paren) => self.visit_discarded(&mut paren.expr),
            Expr::Unary(unary)
                if matches!(
                    unary.op,
                    swc_core::ecma::ast::UnaryOp::Void | swc_core::ecma::ast::UnaryOp::Bang
                ) =>
            {
                self.visit_discarded(&mut unary.arg)
            }
            _ => expr.visit_mut_with(self),
        }
    }

    /// `module.exports = value` for a value export call.
    fn value_export(&mut self, mut call: CallExpr) -> Option<Box<Expr>> {
        if !matches!(
            self.export_target(&call, &ExportCall::Value),
            Some(ExportTarget::Own)
        ) || call.args[0].spread.is_some()
        {
            return None;
        }
        call.args[0].expr.visit_mut_with(self);
        let value = call.args.swap_remove(0).expr;
        let Expr::Member(target) = *member(ident_expr(&self.module_name), "exports") else {
            unreachable!("member builds a member expression");
        };
        Some(Box::new(Expr::Assign(AssignExpr {
            span: Default::default(),
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(target)),
            right: value,
        })))
    }

    /// The optional second argument targets a module id. Outside a merged
    /// group only this factory's own ids are accepted, which the runtime
    /// resolves to the current module; in a merged group the primary id is
    /// the current module and the other members' ESM registrations are
    /// exported under their aliases.
    fn export_target(&self, call: &CallExpr, kind: &ExportCall) -> Option<ExportTarget> {
        let id = match call.args.as_slice() {
            [_] => return self.group.is_none().then_some(ExportTarget::Own),
            [_, ExprOrSpread { spread: None, expr }] => numeric_id_from_expr(expr)?,
            _ => return None,
        };
        match self.group {
            None => self.own_ids.contains(&id).then_some(ExportTarget::Own),
            Some(group) if id == group.primary => Some(ExportTarget::Own),
            Some(group) => (matches!(kind, ExportCall::Esm)
                && group.foreign.iter().any(|facade| facade.id == id))
            .then_some(ExportTarget::Foreign(id)),
        }
    }

    fn translate_export(&mut self, mut call: CallExpr, kind: ExportCall) -> Option<Vec<Stmt>> {
        let target = self.export_target(&call, &kind)?;
        if call.args[0].spread.is_some() {
            return None;
        }
        // Values and getters may themselves use the context.
        call.args[0].expr.visit_mut_with(self);
        let argument = call.args.swap_remove(0).expr;
        match (kind, target) {
            (ExportCall::Value, _) => Some(vec![assign_stmt(
                member(ident_expr(&self.module_name), "exports"),
                argument,
            )]),
            (ExportCall::Esm, ExportTarget::Own) => self.translate_esm_bindings(*argument, None),
            (ExportCall::Esm, ExportTarget::Foreign(id)) => {
                let group = self.group.expect("foreign targets need a group");
                let facade = group.foreign.iter().find(|facade| facade.id == id)?;
                self.translate_esm_bindings(*argument, Some(facade))
            }
        }
    }

    /// `ctx.s([name, getter, ...])` with `name, 0, value` value bindings
    /// (Next 16) or getter-only lists (Next 15.5). A setter, or any other
    /// tag, is not translated.
    fn translate_esm_bindings(&self, bindings: Expr, facade: Option<&Facade>) -> Option<Vec<Stmt>> {
        // A member's binding is exported under its alias.
        let rename = |name: String| -> Option<String> {
            match facade {
                None => Some(name),
                Some(facade) => facade
                    .bindings
                    .iter()
                    .find(|(exported, _)| *exported == name)
                    .map(|(_, alias)| alias.clone()),
            }
        };
        let array = match bindings {
            Expr::Array(array) => array,
            // 15.3–15.4: `{ name: getter }`; a `[getter, setter]` value is
            // not translated.
            Expr::Object(object) => {
                let mut getters = Vec::with_capacity(object.props.len());
                for prop in object.props {
                    let PropOrSpread::Prop(prop) = prop else {
                        return None;
                    };
                    let Prop::KeyValue(KeyValueProp { key, value }) = *prop else {
                        return None;
                    };
                    let name = match key {
                        PropName::Ident(key) => key.sym.to_string(),
                        PropName::Str(key) => key.value.as_str()?.to_string(),
                        _ => return None,
                    };
                    if !is_function(&value) {
                        return None;
                    }
                    getters.push((rename(name)?, value));
                }
                return Some(self.esm_statements(getters, Vec::new()));
            }
            _ => return None,
        };
        let mut elements = Vec::with_capacity(array.elems.len());
        for elem in array.elems {
            let ExprOrSpread { spread: None, expr } = elem? else {
                return None;
            };
            elements.push(expr);
        }

        let mut getters = Vec::new();
        let mut values = Vec::new();
        let mut elements = elements.into_iter().peekable();
        while let Some(name) = elements.next() {
            let Expr::Lit(Lit::Str(name)) = *name else {
                return None;
            };
            let name = name.value.as_str()?.to_string();
            let next = elements.next()?;
            let name = rename(name)?;
            if matches!(&*next, Expr::Lit(Lit::Num(Number { value, .. })) if *value == 0.0) {
                values.push((name, elements.next()?));
                continue;
            }
            if !is_function(&next) {
                return None;
            }
            if elements
                .peek()
                .is_some_and(|following| is_function(following))
            {
                return None;
            }
            getters.push((name, next));
        }
        Some(self.esm_statements(getters, values))
    }

    /// `require.r(exports)`, `require.d(exports, { getters })`, then value
    /// assignments in their original order.
    fn esm_statements(
        &self,
        getters: Vec<(String, Box<Expr>)>,
        values: Vec<(String, Box<Expr>)>,
    ) -> Vec<Stmt> {
        let exports = || ident_expr(&self.exports_name);
        let mut stmts = vec![expr_stmt(call_expr(
            member(ident_expr(&self.ctx), "r"),
            vec![exports()],
        ))];
        if !getters.is_empty() {
            let object = ObjectLit {
                span: Default::default(),
                props: getters
                    .into_iter()
                    .map(|(name, getter)| {
                        PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                            key: prop_name(&name),
                            value: getter,
                        })))
                    })
                    .collect(),
            };
            stmts.push(expr_stmt(call_expr(
                member(ident_expr(&self.ctx), "d"),
                vec![exports(), Box::new(Expr::Object(object))],
            )));
        }
        for (name, value) in values {
            stmts.push(assign_stmt(member(exports(), &name), value));
        }
        stmts
    }

    fn resolve_alias(&self, id: usize) -> usize {
        self.aliases.get(&id).copied().unwrap_or(id)
    }

    /// `ctx(<id>)`, the webpack require call the normalizer rewrites.
    fn require_call(&self, id: usize) -> Box<Expr> {
        call_expr(
            ident_expr(&self.ctx),
            vec![number_expr(self.resolve_alias(id))],
        )
    }

    /// `ctx.A(loader)` (or 15.3's `ctx.r(loader)(ctx.i)`): inline a loader of
    /// this input; otherwise call the translated loader module, which the
    /// multi-input numeric rewrite can reach in another chunk.
    fn loader_call(&mut self, id: usize) -> Box<Expr> {
        match self.loaders.get(&self.resolve_alias(id)).copied() {
            Some(target) => self.loader_expr(target),
            None => call_expr(self.require_call(id), Vec::new()),
        }
    }

    fn translate_call(&mut self, call: &mut CallExpr) -> Option<Box<Expr>> {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let letter = self.ctx_member(callee)?.to_owned();
        if let Some(residual) = self.residual_member(&letter) {
            call.callee = Callee::Expr(residual);
            call.args.visit_mut_with(self);
            return Some(Box::new(Expr::Call(std::mem::take(call))));
        }
        let translated = match (letter.as_str(), call.args.as_mut_slice()) {
            ("r" | "i", [ExprOrSpread { spread: None, expr }]) => {
                match numeric_id_from_expr(expr) {
                    Some(id) => Some(self.require_call(id)),
                    // A computed id stays a runtime require by id.
                    None => {
                        expr.visit_mut_with(self);
                        Some(call_expr(ident_expr(&self.ctx), vec![expr.clone()]))
                    }
                }
            }
            ("A", [ExprOrSpread { spread: None, expr }]) => {
                numeric_id_from_expr(expr).map(|id| self.loader_call(id))
            }
            ("f", [ExprOrSpread { spread: None, expr }]) => {
                expr.visit_mut_with(self);
                Some(call_expr(self.context_helper_expr(), vec![expr.clone()]))
            }
            (
                "x",
                [ExprOrSpread {
                    spread: None,
                    expr: request,
                }, ExprOrSpread {
                    spread: None,
                    expr: thunk,
                }],
            ) => self
                .external_request(request, thunk)
                .map(|request| call_expr(ident_expr(&self.ctx), vec![request])),
            _ => None,
        };
        if translated.is_none() {
            self.reject(Some(&letter));
        }
        translated
    }

    /// `__turbopack_context__.<letter>` for a runtime member that reaches
    /// no module graph or export, so the call is kept instead of making the
    /// whole factory opaque: chunk loading (`l`, `L`), path and file URL
    /// resolution (`P`, `F`), the host `require` (`t`), and the throwing
    /// require stub (`z`). Each letter kept its meaning across every
    /// accepted release; `F` first appears in 16.3.
    fn residual_member(&mut self, letter: &str) -> Option<Box<Expr>> {
        let mut chars = letter.chars();
        let letter @ ('l' | 'L' | 'P' | 'F' | 't' | 'z') = chars.next()? else {
            return None;
        };
        if chars.next().is_some() {
            return None;
        }
        self.residual.get_or_insert(letter);
        Some(member(
            ident_expr(&Atom::from(RESIDUAL_CONTEXT)),
            &letter.to_string(),
        ))
    }

    /// `ctx.<letter>.bind(ctx)`, which Turbopack emits for a runtime
    /// function used as a value (App Router server page entries pass
    /// `ctx.r` and `ctx.l` this way). The bound require is the require
    /// parameter; a residual member stays bound to the residual name.
    fn translate_bound_member(&mut self, expr: &Expr) -> Option<Box<Expr>> {
        let Expr::Call(call) = expr else {
            return None;
        };
        let [ExprOrSpread {
            spread: None,
            expr: receiver,
        }] = call.args.as_slice()
        else {
            return None;
        };
        if !self.is_ctx(receiver) {
            return None;
        }
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(bind),
            ..
        }) = strip_parens(callee)
        else {
            return None;
        };
        if bind.sym != "bind" {
            return None;
        }
        let letter = self.ctx_member(obj)?;
        if letter == "r" {
            return Some(ident_expr(&self.ctx));
        }
        if letter == "f" {
            return Some(self.context_helper_expr());
        }
        let residual = self.residual_member(letter)?;
        Some(call_expr(
            member(residual, "bind"),
            vec![ident_expr(&Atom::from(RESIDUAL_CONTEXT))],
        ))
    }

    fn context_helper_expr(&mut self) -> Box<Expr> {
        self.uses_context_helper = true;
        ident_expr(&self.context_helper)
    }

    /// `ctx.f({ key: { id, module } })("key")` with a listed constant key,
    /// which Next.js uses to resolve its instrumentation hook, becomes that
    /// entry's `module` body. The runtime calls `map[key].module()`; the
    /// other entries are object literals of arrow functions, so dropping
    /// them has no effect.
    fn translate_constant_context_call(&mut self, expr: &Expr) -> Option<Box<Expr>> {
        let Expr::Call(call) = expr else {
            return None;
        };
        let [ExprOrSpread {
            spread: None,
            expr: request,
        }] = call.args.as_slice()
        else {
            return None;
        };
        let Expr::Lit(Lit::Str(request)) = strip_parens(request) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Call(context) = strip_parens(callee) else {
            return None;
        };
        let Callee::Expr(context_callee) = &context.callee else {
            return None;
        };
        if self.ctx_member(context_callee) != Some("f") {
            return None;
        }
        let [ExprOrSpread {
            spread: None,
            expr: map,
        }] = context.args.as_slice()
        else {
            return None;
        };
        let Expr::Object(map) = strip_parens(map) else {
            return None;
        };
        let mut selected = None;
        for prop in &map.props {
            let PropOrSpread::Prop(prop) = prop else {
                return None;
            };
            let Prop::KeyValue(KeyValueProp {
                key: PropName::Str(key),
                value,
            }) = &**prop
            else {
                return None;
            };
            let module = context_entry_module(value)?;
            if key.value == request.value {
                // A repeated key keeps the last entry, as in any object literal.
                selected = Some(module);
            }
        }
        let mut module = Box::new(selected?.clone());
        module.visit_mut_with(self);
        Some(module)
    }

    /// 15.3–15.4 call an async loader inline: `ctx.r(loader)(ctx.i)`.
    fn translate_inline_loader_call(&mut self, expr: &Expr) -> Option<Box<Expr>> {
        let Expr::Call(call) = expr else {
            return None;
        };
        let [ExprOrSpread {
            spread: None,
            expr: importer,
        }] = call.args.as_slice()
        else {
            return None;
        };
        if self.ctx_member(importer) != Some("i") {
            return None;
        }
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Call(require) = strip_parens(callee) else {
            return None;
        };
        let Callee::Expr(require_callee) = &require.callee else {
            return None;
        };
        if self.ctx_member(require_callee) != Some("r") {
            return None;
        }
        let [ExprOrSpread {
            spread: None,
            expr: id,
        }] = require.args.as_slice()
        else {
            return None;
        };
        Some(self.loader_call(numeric_id_from_expr(id)?))
    }

    fn loader_expr(&mut self, target: LoaderTarget) -> Box<Expr> {
        self.uses_promise = true;
        match target {
            LoaderTarget::Module(target) => {
                let load = Box::new(Expr::Arrow(ArrowExpr {
                    body: Box::new(ArrowFunctionBody::Expr(self.require_call(target))),
                    ..Default::default()
                }));
                call_expr(member(promise_resolve(), "then"), vec![load])
            }
            LoaderTarget::Empty => promise_resolve(),
        }
    }

    /// `ctx.x("name", () => require("name"))`, a server external that the
    /// runtime resolves through the host `require`.
    fn external_request(&self, request: &Expr, thunk: &Expr) -> Option<Box<Expr>> {
        let Expr::Lit(Lit::Str(name)) = strip_parens(request) else {
            return None;
        };
        let (params, Some(body)) = function_returning_or_empty(thunk)? else {
            return None;
        };
        if !params.is_empty() {
            return None;
        }
        let Expr::Call(call) = strip_parens(body) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        if !matches!(strip_parens(callee), Expr::Ident(require) if require.sym == "require" && require.ctxt.outer() == self.unresolved_mark)
        {
            return None;
        }
        let [ExprOrSpread {
            spread: None,
            expr: argument,
        }] = call.args.as_slice()
        else {
            return None;
        };
        let Expr::Lit(Lit::Str(argument)) = strip_parens(argument) else {
            return None;
        };
        (argument.value == name.value).then(|| Box::new(Expr::Lit(Lit::Str(name.clone()))))
    }
}

impl VisitMut for ContextTranslator<'_> {
    fn visit_mut_expr_stmt(&mut self, stmt: &mut ExprStmt) {
        self.visit_discarded(&mut stmt.expr);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if self.failure.is_some() {
            return;
        }
        if let Some(replacement) = self.translate_inline_loader_call(expr) {
            *expr = *replacement;
            return;
        }
        if let Some(replacement) = self.translate_bound_member(expr) {
            *expr = *replacement;
            return;
        }
        if let Some(replacement) = self.translate_constant_context_call(expr) {
            *expr = *replacement;
            return;
        }
        if let Expr::Call(call) = expr {
            let is_ctx_call =
                matches!(&call.callee, Callee::Expr(callee) if self.ctx_member(callee).is_some());
            if is_ctx_call {
                if let Some(replacement) = self.translate_call(call) {
                    *expr = *replacement;
                }
                return;
            }
        }
        if let Some(letter) = self.ctx_member(expr) {
            if letter == "g" {
                self.uses_global_this = true;
                *expr = *ident_expr(&Atom::from("globalThis"));
            } else if letter == "e" {
                *expr = *ident_expr(&self.exports_name);
            } else if letter == "m" {
                *expr = *ident_expr(&self.module_name);
            } else if letter == "r" {
                // The AMD `define` wrapper passes the require function to the
                // factory, as webpack passes `__webpack_require__`. From 15.5
                // the runtime method reads `this`, so a factory that called
                // this detached value would throw where the translation
                // keeps working.
                *expr = *ident_expr(&self.ctx);
            } else if letter == "f" {
                *expr = *self.context_helper_expr();
            } else if let Some(residual) = self.residual_member(letter) {
                *expr = *residual;
            } else {
                let letter = letter.to_owned();
                self.reject(Some(&letter));
            }
            return;
        }
        if self.is_ctx(expr) {
            // The context escapes as a value.
            self.reject(None);
            return;
        }
        expr.visit_mut_children_with(self);
    }
}

/// The `module` body of a `require.context` entry
/// `{ id: () => <id>, module: () => <expr> }`.
fn context_entry_module(entry: &Expr) -> Option<&Expr> {
    let Expr::Object(entry) = strip_parens(entry) else {
        return None;
    };
    let mut module = None;
    let mut has_id = false;
    for prop in &entry.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(KeyValueProp {
            key: PropName::Ident(key),
            value,
        }) = &**prop
        else {
            return None;
        };
        let (params, body) = function_returning(value)?;
        if !params.is_empty() {
            return None;
        }
        match key.sym.as_ref() {
            "id" if !has_id => has_id = true,
            "module" if module.is_none() => module = Some(body),
            _ => return None,
        }
    }
    module.filter(|_| has_id)
}

/// A copy of the runtime's `require.context` implementation. From 16.1 the
/// runtime drops a `?query` or `#fragment` from the request before the
/// lookup (`parseRequest`); context keys never contain one.
const MODULE_CONTEXT_HELPER: &str = r#"function __HELPER__(map) {__REQUEST_FN__
  function context(id) {__REQUEST__
    if (Object.prototype.hasOwnProperty.call(map, id)) return map[id].module();
    const error = new Error(`Cannot find module '${id}'`);
    error.code = "MODULE_NOT_FOUND";
    throw error;
  }
  context.keys = () => Object.keys(map);
  context.resolve = (id) => {__REQUEST__
    if (Object.prototype.hasOwnProperty.call(map, id)) return map[id].id();
    const error = new Error(`Cannot find module '${id}'`);
    error.code = "MODULE_NOT_FOUND";
    throw error;
  };
  context.import = async (id) => await context(id);
  return context;
}"#;

const MODULE_CONTEXT_REQUEST: &str = r##"
  function request(id) {
    const hash = id.indexOf("#");
    if (hash !== -1) id = id.substring(0, hash);
    const query = id.indexOf("?");
    if (query !== -1) id = id.substring(0, query);
    return id;
  }"##;

/// The helper with 16.1+ request parsing, or without it for a chunk that
/// proves an older runtime. A flat chunk gets the 16.1+ form: 16.0 and 16.1
/// emit identical chunks, and the newer behavior differs only for a request
/// that contains `?` or `#`.
fn module_context_helper(name: &Atom, parses_request: bool) -> Stmt {
    struct ClearSpans;
    impl VisitMut for ClearSpans {
        fn visit_mut_span(&mut self, span: &mut Span) {
            *span = DUMMY_SP;
        }
    }
    let cm: Lrc<SourceMap> = Default::default();
    let (request_fn, request) = if parses_request {
        (MODULE_CONTEXT_REQUEST, "\n    id = request(id);")
    } else {
        ("", "")
    };
    let source = MODULE_CONTEXT_HELPER
        .replace("__HELPER__", name)
        .replace("__REQUEST_FN__", request_fn)
        .replace("__REQUEST__", request);
    let fm = cm.new_source_file(FileName::Anon.into(), source);
    let mut parser = Parser::new(
        Syntax::Es(Default::default()),
        StringInput::from(&*fm),
        None,
    );
    let mut stmt = parser
        .parse_stmt_list_item()
        .expect("the module context helper parses");
    stmt.visit_mut_with(&mut ClearSpans);
    stmt
}

/// `(params => body)(args)` with an expression body.
fn is_arrow_iife(call: &CallExpr) -> bool {
    matches!(&call.callee, Callee::Expr(callee) if matches!(
        strip_parens(callee),
        Expr::Arrow(ArrowExpr { body, .. }) if matches!(**body, ArrowFunctionBody::Expr(_))
    ))
}

fn is_function(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Arrow(_) | Expr::Fn(_))
}

fn ident_expr(sym: &Atom) -> Box<Expr> {
    Box::new(Expr::Ident(Ident::new_no_ctxt(
        sym.clone(),
        Default::default(),
    )))
}

fn member(object: Box<Expr>, name: &str) -> Box<Expr> {
    let prop = if is_valid_identifier_name(name) {
        MemberProp::Ident(IdentName::new(name.into(), Default::default()))
    } else {
        MemberProp::Computed(ComputedPropName {
            span: Default::default(),
            expr: Box::new(Expr::Lit(Lit::Str(Str {
                span: Default::default(),
                value: name.into(),
                raw: None,
            }))),
        })
    };
    Box::new(Expr::Member(MemberExpr {
        span: Default::default(),
        obj: object,
        prop,
    }))
}

fn prop_name(name: &str) -> PropName {
    if is_valid_identifier_name(name) {
        PropName::Ident(IdentName::new(name.into(), Default::default()))
    } else {
        PropName::Str(Str {
            span: Default::default(),
            value: name.into(),
            raw: None,
        })
    }
}

fn call_expr(callee: Box<Expr>, args: Vec<Box<Expr>>) -> Box<Expr> {
    Box::new(Expr::Call(CallExpr {
        callee: Callee::Expr(callee),
        args: args
            .into_iter()
            .map(|expr| ExprOrSpread { spread: None, expr })
            .collect(),
        ..Default::default()
    }))
}

fn promise_resolve() -> Box<Expr> {
    call_expr(
        member(ident_expr(&Atom::from("Promise")), "resolve"),
        Vec::new(),
    )
}

fn expr_stmt(expr: Box<Expr>) -> Stmt {
    Stmt::Expr(ExprStmt {
        span: Default::default(),
        expr,
    })
}

fn assign_stmt(target: Box<Expr>, value: Box<Expr>) -> Stmt {
    let Expr::Member(target) = *target else {
        unreachable!("assignment targets are member expressions");
    };
    expr_stmt(Box::new(Expr::Assign(AssignExpr {
        span: Default::default(),
        op: AssignOp::Assign,
        left: AssignTarget::Simple(SimpleAssignTarget::Member(target)),
        right: value,
    })))
}
