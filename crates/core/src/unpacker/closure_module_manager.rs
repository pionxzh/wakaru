//! Closure Library `ModuleManager` / Google gstatic bundle extraction.
//!
//! These bundles are not CommonJS or ESM. Their modules execute against a
//! shared, compiler-selected namespace object and are commonly wrapped in
//! guarded `try` blocks. Keep that wrapper and the loader calls intact instead
//! of inventing imports or exports that the input did not contain. Reject any
//! unguarded statement whose placement cannot be preserved without guessing.

use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, Span, Spanned, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, BlockStmt, BlockStmtOrExpr, CallExpr, Callee, EsVersion, Expr, ExprOrSpread, FnExpr,
    Lit, MemberProp, Module, ModuleItem, Pat, Stmt, Str, TryStmt, UnaryOp,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::unpacker::{
    generated_source_map_points, sanitize_relative_path, span_byte_range, spans_byte_ranges,
    BundleFormat, GeneratedSourceMapPoint, UnpackResult, UnpackedModule,
};
use crate::utils::paren::strip_parens;

const INITIALIZER_EXPORT: &str = "_ModuleManager_initialize";
const DUMP_EXCEPTION_EXPORT: &str = "_DumpException";

pub(super) fn detect_from_module(
    module: &Module,
    cm: Lrc<SourceMap>,
    source: &str,
) -> Option<UnpackResult> {
    let body = module_body_view(module)?;
    let initializer = collect_initializer(module)?;
    let known_ids = initializer
        .as_ref()
        .map(Initializer::known_ids)
        .unwrap_or_default();

    let mut candidates = collect_segment_candidates(&body, &cm, source, &known_ids)?;
    if candidates.is_empty() {
        return None;
    }

    // Without the exported ModuleManager initializer, require an annotation on
    // every segment. This is the conservative chunk-response path: a lone
    // application try/catch must not become a bundle merely because it reports
    // through `_DumpException`.
    if initializer.is_none()
        && candidates
            .iter()
            .any(|candidate| candidate.marker.is_none())
    {
        return None;
    }

    assign_segment_ids(&mut candidates, initializer.as_ref())?;
    validate_boundary_helpers(&candidates, initializer.as_ref())?;

    let graph_ids = initializer
        .as_ref()
        .map(|initializer| {
            initializer
                .graph
                .iter()
                .map(|module| module.id.as_str())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    if !graph_ids.is_empty()
        && candidates.iter().any(|candidate| {
            candidate
                .id
                .as_deref()
                .is_some_and(|id| !graph_ids.contains(id))
        })
    {
        return None;
    }

    let loading_ids = initializer
        .as_ref()
        .map(|initializer| initializer.loading_ids.as_slice())
        .unwrap_or_default();
    let entry_id = initializer
        .as_ref()
        .and_then(|_| initializer_owner_id(&candidates))
        .or_else(|| loading_ids.first().cloned())
        .or_else(|| {
            candidates
                .first()
                .and_then(|candidate| candidate.id.clone())
        });

    let mut filenames = HashSet::new();
    let mut modules = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let candidate_source_range = candidate.source_range(&cm)?;
        let id = candidate.id?;
        let filename = module_filename(&id, &mut filenames);
        let emitted = body
            .emit_segment(candidate.stmt.as_ref(), cm.clone())
            .ok()?;
        let mut source_ranges = body.shared_source_ranges(&cm);
        source_ranges.push(candidate_source_range);
        source_ranges.sort_unstable();
        source_ranges = coalesce_ranges(source_ranges);

        modules.push(UnpackedModule {
            is_entry: entry_id.as_deref() == Some(id.as_str()),
            id,
            code: emitted.code,
            filename,
            source_ranges,
            source_input: String::new(),
            generated_source_map: emitted.source_map,
        });
    }

    // These files remain shared-namespace fragments. In particular, loader
    // graph edges are not ESM edges and must not trigger cycle pre-merging.
    Some(UnpackResult::without_cycle_premerge(
        modules,
        BundleFormat::ClosureModuleManager,
    ))
}

#[derive(Clone)]
struct Initializer {
    graph: Vec<GraphModule>,
    loading_ids: Vec<String>,
}

impl Initializer {
    fn known_ids(&self) -> HashSet<String> {
        self.graph
            .iter()
            .map(|module| module.id.clone())
            .chain(self.loading_ids.iter().cloned())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GraphModule {
    id: String,
    dependencies: Vec<String>,
}

fn decode_module_graph(encoded: &str) -> Option<Vec<GraphModule>> {
    if encoded.is_empty() {
        return Some(Vec::new());
    }

    let mut modules: Vec<GraphModule> = Vec::new();
    let mut seen = HashSet::new();
    for record in encoded.split('/') {
        let (id, encoded_dependencies) = record
            .split_once(':')
            .map_or((record, None), |(id, deps)| (id, Some(deps)));
        if !is_valid_module_id(id) || !seen.insert(id.to_string()) {
            return None;
        }

        let dependencies = match encoded_dependencies {
            None | Some("") => Vec::new(),
            Some(dependencies) => dependencies
                .split(',')
                .map(|index| {
                    if index.is_empty() || !index.chars().all(|ch| ch.is_ascii_alphanumeric()) {
                        return None;
                    }
                    let index = usize::from_str_radix(index, 36).ok()?;
                    modules.get(index).map(|module| module.id.clone())
                })
                .collect::<Option<Vec<_>>>()?,
        };
        modules.push(GraphModule {
            id: id.to_string(),
            dependencies,
        });
    }
    Some(modules)
}

#[derive(Default)]
struct InitializerCollector {
    initializers: Vec<Initializer>,
    invalid: bool,
}

impl Visit for InitializerCollector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if callee_export_name(call).as_deref() == Some(INITIALIZER_EXPORT) {
            match parse_initializer_call(call) {
                Some(initializer) => self.initializers.push(initializer),
                None => self.invalid = true,
            }
        }
        call.visit_children_with(self);
    }
}

fn collect_initializer(module: &Module) -> Option<Option<Initializer>> {
    let mut collector = InitializerCollector::default();
    module.visit_with(&mut collector);
    if collector.invalid || collector.initializers.len() > 1 {
        return None;
    }
    Some(collector.initializers.pop())
}

fn parse_initializer_call(call: &CallExpr) -> Option<Initializer> {
    if !(1..=2).contains(&call.args.len()) {
        return None;
    }
    let graph = string_arg(call.args.first()?)?;
    let graph = decode_module_graph(&graph)?;
    let loading_ids = match call.args.get(1) {
        Some(arg) => string_array_arg(arg)?,
        None => Vec::new(),
    };

    if !graph.is_empty() {
        let graph_ids = graph
            .iter()
            .map(|module| module.id.as_str())
            .collect::<HashSet<_>>();
        if loading_ids
            .iter()
            .any(|id| !graph_ids.contains(id.as_str()))
        {
            return None;
        }
    }
    if loading_ids.iter().any(|id| !is_valid_module_id(id)) {
        return None;
    }

    Some(Initializer { graph, loading_ids })
}

fn initializer_owner_id(candidates: &[SegmentCandidate]) -> Option<String> {
    candidates
        .iter()
        .find(|candidate| candidate.contains_initializer)
        .and_then(|candidate| candidate.id.clone())
}

fn string_array_arg(arg: &ExprOrSpread) -> Option<Vec<String>> {
    if arg.spread.is_some() {
        return None;
    }
    let Expr::Array(ArrayLit { elems, .. }) = strip_parens(&arg.expr) else {
        return None;
    };
    elems
        .iter()
        .map(|element| string_arg(element.as_ref()?))
        .collect()
}

fn string_arg(arg: &ExprOrSpread) -> Option<String> {
    if arg.spread.is_some() {
        return None;
    }
    let Expr::Lit(Lit::Str(value)) = strip_parens(&arg.expr) else {
        return None;
    };
    Some(value.value.to_string_lossy().to_string())
}

#[derive(Clone)]
struct ModuleBodyView {
    statements: Vec<Stmt>,
    statement_floor: u32,
    preserved_prefix_len: usize,
    render: SegmentRender,
}

impl ModuleBodyView {
    fn emit_segment(
        &self,
        segment: Option<&Stmt>,
        cm: Lrc<SourceMap>,
    ) -> anyhow::Result<EmittedSegment> {
        let module = match &self.render {
            SegmentRender::Direct => Module {
                span: DUMMY_SP,
                body: segment.cloned().map(ModuleItem::Stmt).into_iter().collect(),
                shebang: None,
            },
            SegmentRender::Wrapper {
                template_items,
                wrapper_index,
                body_span,
                body_prelude,
                ..
            } => {
                let mut template_items = template_items.clone();
                let ModuleItem::Stmt(outer_stmt) = &mut template_items[*wrapper_index] else {
                    return Err(anyhow::anyhow!("Closure outer wrapper was not a statement"));
                };
                let mut replacer = WrapperBodyReplacer {
                    target: *body_span,
                    replacement: body_prelude
                        .iter()
                        .cloned()
                        .chain(segment.cloned())
                        .collect(),
                    replaced: false,
                };
                outer_stmt.visit_mut_with(&mut replacer);
                if !replacer.replaced {
                    return Err(anyhow::anyhow!("failed to preserve Closure outer wrapper"));
                }
                Module {
                    span: DUMMY_SP,
                    body: template_items,
                    shebang: None,
                }
            }
        };
        emit_module(&module, cm)
    }

    fn shared_source_ranges(&self, cm: &SourceMap) -> Vec<(u32, u32)> {
        let SegmentRender::Wrapper {
            shared_source_spans,
            ..
        } = &self.render
        else {
            return Vec::new();
        };
        spans_byte_ranges(cm, shared_source_spans.iter().copied())
    }
}

#[derive(Clone)]
enum SegmentRender {
    Direct,
    Wrapper {
        template_items: Vec<ModuleItem>,
        wrapper_index: usize,
        body_span: Span,
        body_prelude: Vec<Stmt>,
        shared_source_spans: Vec<Span>,
    },
}

fn module_body_view(module: &Module) -> Option<ModuleBodyView> {
    let wrappers = module
        .body
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
                return None;
            };
            invoked_function_body(expr_stmt.expr.as_ref()).map(|body| (index, body))
        })
        .collect::<Vec<_>>();
    if let [(wrapper_index, body)] = wrappers.as_slice() {
        if module.body[*wrapper_index + 1..]
            .iter()
            .all(|item| matches!(item, ModuleItem::Stmt(Stmt::Empty(_))))
        {
            let body_prelude = body
                .stmts
                .iter()
                .take_while(|stmt| !matches!(stmt, Stmt::Try(_)))
                .cloned()
                .collect::<Vec<_>>();
            let shared_source_spans = module.body[..*wrapper_index]
                .iter()
                .map(Spanned::span)
                .chain(body_prelude.iter().map(Spanned::span))
                .collect();
            let mut template_items = module.body[..=*wrapper_index].to_vec();
            let ModuleItem::Stmt(template_wrapper) = &mut template_items[*wrapper_index] else {
                return None;
            };
            let mut template_replacer = WrapperBodyReplacer {
                target: body.span,
                replacement: body_prelude.clone(),
                replaced: false,
            };
            template_wrapper.visit_mut_with(&mut template_replacer);
            if !template_replacer.replaced {
                return None;
            }
            return Some(ModuleBodyView {
                statements: body.stmts.clone(),
                statement_floor: 0,
                preserved_prefix_len: body_prelude.len(),
                render: SegmentRender::Wrapper {
                    template_items,
                    wrapper_index: *wrapper_index,
                    body_span: body.span,
                    body_prelude,
                    shared_source_spans,
                },
            });
        }
    }

    let statements = module
        .body
        .iter()
        .map(|item| match item {
            ModuleItem::Stmt(stmt) => Some(stmt.clone()),
            ModuleItem::ModuleDecl(_) => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some(ModuleBodyView {
        statements,
        statement_floor: 0,
        preserved_prefix_len: 0,
        render: SegmentRender::Direct,
    })
}

fn invoked_function_body(expr: &Expr) -> Option<&BlockStmt> {
    match strip_parens(expr) {
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => invoked_function_body(&unary.arg),
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            match strip_parens(callee) {
                Expr::Fn(FnExpr { function, .. }) => function.body.as_ref(),
                Expr::Arrow(arrow) => match arrow.body.as_ref() {
                    BlockStmtOrExpr::BlockStmt(body) => Some(body),
                    BlockStmtOrExpr::Expr(_) => None,
                },
                Expr::Member(member)
                    if member_prop_name(&member.prop).as_deref() == Some("call") =>
                {
                    function_expr_body(&member.obj)
                }
                _ => {
                    let mut function_bodies = call
                        .args
                        .iter()
                        .filter(|arg| arg.spread.is_none())
                        .filter_map(|arg| function_expr_body(&arg.expr));
                    let body = function_bodies.next()?;
                    function_bodies.next().is_none().then_some(body)
                }
            }
        }
        _ => None,
    }
}

fn function_expr_body(expr: &Expr) -> Option<&BlockStmt> {
    match strip_parens(expr) {
        Expr::Fn(FnExpr { function, .. }) => function.body.as_ref(),
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(body) => Some(body),
            BlockStmtOrExpr::Expr(_) => None,
        },
        _ => None,
    }
}

struct WrapperBodyReplacer {
    target: Span,
    replacement: Vec<Stmt>,
    replaced: bool,
}

fn coalesce_ranges(ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    let mut coalesced: Vec<(u32, u32)> = Vec::new();
    for (start, end) in ranges {
        match coalesced.last_mut() {
            Some(previous) if start <= previous.1 => previous.1 = previous.1.max(end),
            _ => coalesced.push((start, end)),
        }
    }
    coalesced
}

impl VisitMut for WrapperBodyReplacer {
    fn visit_mut_block_stmt(&mut self, block: &mut BlockStmt) {
        if block.span == self.target {
            block.stmts = std::mem::take(&mut self.replacement);
            self.replaced = true;
            return;
        }
        block.visit_mut_children_with(self);
    }
}

#[derive(Clone)]
struct Marker {
    id: String,
    start: u32,
    end: u32,
}

#[derive(Clone)]
struct SegmentCandidate {
    stmt: Option<Stmt>,
    marker: Option<Marker>,
    contains_initializer: bool,
    id: Option<String>,
    begin_helper: Option<String>,
    end_helper: Option<String>,
}

impl SegmentCandidate {
    fn source_range(&self, cm: &SourceMap) -> Option<(u32, u32)> {
        match (&self.marker, &self.stmt) {
            (Some(marker), Some(stmt)) => {
                let (_, end) = span_byte_range(cm, stmt.span())?;
                Some((marker.start, end))
            }
            (Some(marker), None) => Some((marker.start, marker.end)),
            (None, Some(stmt)) => span_byte_range(cm, stmt.span()),
            (None, None) => None,
        }
    }
}

fn collect_segment_candidates(
    body: &ModuleBodyView,
    cm: &SourceMap,
    source: &str,
    known_ids: &HashSet<String>,
) -> Option<Vec<SegmentCandidate>> {
    let mut previous_end = body.statement_floor;
    let mut candidates = Vec::new();

    for (index, stmt) in body.statements.iter().enumerate() {
        let (stmt_start, stmt_end) = span_byte_range(cm, stmt.span())?;
        let markers = markers_before_statement(source, previous_end, stmt_start)?;
        previous_end = stmt_end;

        let Stmt::Try(try_stmt) = stmt else {
            if !markers.is_empty() {
                return None;
            }
            if index < body.preserved_prefix_len || matches!(stmt, Stmt::Empty(_)) {
                continue;
            }
            return None;
        };
        if !has_dump_exception_handler(try_stmt) {
            return None;
        }

        let marker = markers.last().cloned();
        for empty_marker in markers
            .iter()
            .take(markers.len().saturating_sub(1))
            .cloned()
        {
            candidates.push(SegmentCandidate {
                stmt: None,
                id: Some(empty_marker.id.clone()),
                marker: Some(empty_marker),
                contains_initializer: false,
                begin_helper: None,
                end_helper: None,
            });
        }

        let mut allowed_ids = known_ids.clone();
        if let Some(marker) = &marker {
            allowed_ids.insert(marker.id.clone());
        }
        let boundary = infer_loader_boundary(try_stmt, &allowed_ids);
        if let (Some(marker), Some(boundary)) = (&marker, &boundary) {
            if marker.id != boundary.id {
                return None;
            }
        }

        candidates.push(SegmentCandidate {
            stmt: Some(stmt.clone()),
            id: marker
                .as_ref()
                .map(|marker| marker.id.clone())
                .or_else(|| boundary.as_ref().map(|boundary| boundary.id.clone())),
            marker,
            contains_initializer: statement_contains_initializer(stmt),
            begin_helper: boundary
                .as_ref()
                .map(|boundary| boundary.begin_helper.clone()),
            end_helper: boundary.map(|boundary| boundary.end_helper),
        });
    }
    Some(candidates)
}

fn markers_before_statement(source: &str, floor: u32, statement_start: u32) -> Option<Vec<Marker>> {
    let floor = floor as usize;
    let statement_start = statement_start as usize;
    if floor > statement_start || statement_start > source.len() {
        return None;
    }
    let gap = &source[floor..statement_start];
    let mut markers = Vec::new();
    let mut cursor = 0;
    let mut previous_marker_end = None;
    while let Some(relative_start) = gap[cursor..].find("/*_M:") {
        let marker_start = cursor + relative_start;
        if previous_marker_end.is_some_and(|end| !gap[end..marker_start].trim().is_empty()) {
            return None;
        }
        let marker_body_start = marker_start + "/*_M:".len();
        let marker_body_end = gap[marker_body_start..].find("*/")? + marker_body_start;
        let marker_end = marker_body_end + 2;
        let id = gap[marker_body_start..marker_body_end].trim();
        if !is_valid_module_id(id) {
            return None;
        }
        markers.push(Marker {
            id: id.to_string(),
            start: (floor + marker_start) as u32,
            end: (floor + marker_end) as u32,
        });
        cursor = marker_end;
        previous_marker_end = Some(marker_end);
    }
    if previous_marker_end.is_some_and(|end| !gap[end..].trim().is_empty()) {
        return None;
    }
    Some(markers)
}

fn is_valid_module_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn has_dump_exception_handler(try_stmt: &TryStmt) -> bool {
    if try_stmt.finalizer.is_some() {
        return false;
    }
    let Some(handler) = &try_stmt.handler else {
        return false;
    };
    let Some(Pat::Ident(caught)) = handler.param.as_ref() else {
        return false;
    };
    let [Stmt::Expr(expr_stmt)] = handler.body.stmts.as_slice() else {
        return false;
    };
    let Expr::Call(call) = strip_parens(&expr_stmt.expr) else {
        return false;
    };
    if callee_export_name(call).as_deref() != Some(DUMP_EXCEPTION_EXPORT) || call.args.len() != 1 {
        return false;
    }
    let arg = &call.args[0];
    arg.spread.is_none()
        && matches!(strip_parens(&arg.expr), Expr::Ident(id) if id.sym == caught.id.sym)
}

struct LoaderBoundary {
    id: String,
    begin_helper: String,
    end_helper: String,
}

fn infer_loader_boundary(
    try_stmt: &TryStmt,
    allowed_ids: &HashSet<String>,
) -> Option<LoaderBoundary> {
    if allowed_ids.is_empty() {
        return None;
    }
    let end_call = try_stmt.block.stmts.last().and_then(top_level_call)?;
    let end_helper = call_target_key(end_call)?;
    let end_ids = call_string_args(end_call)
        .into_iter()
        .filter(|id| allowed_ids.contains(id))
        .collect::<HashSet<_>>();

    for stmt in &try_stmt.block.stmts {
        let Some(call) = top_level_call(stmt) else {
            continue;
        };
        for id in call_string_args(call) {
            if !allowed_ids.contains(&id) {
                continue;
            }
            if !end_call.args.is_empty() && !end_ids.contains(id.as_str()) {
                continue;
            }
            return Some(LoaderBoundary {
                id,
                begin_helper: call_target_key(call)?,
                end_helper,
            });
        }
    }
    None
}

fn call_string_args(call: &CallExpr) -> Vec<String> {
    call.args.iter().filter_map(string_arg).collect()
}

fn top_level_call(stmt: &Stmt) -> Option<&CallExpr> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Call(call) = strip_parens(&expr_stmt.expr) else {
        return None;
    };
    Some(call)
}

fn assign_segment_ids(
    candidates: &mut [SegmentCandidate],
    initializer: Option<&Initializer>,
) -> Option<()> {
    let markerless = candidates
        .iter()
        .all(|candidate| candidate.marker.is_none());
    let positional_ids = markerless
        .then(|| proven_response_order(candidates, initializer))
        .flatten();
    if markerless
        && initializer.is_some_and(|initializer| !initializer.loading_ids.is_empty())
        && positional_ids
            .as_ref()
            .is_none_or(|ids| ids.len() != candidates.len())
    {
        return None;
    }

    let mut seen = HashSet::new();
    for (index, candidate) in candidates.iter_mut().enumerate() {
        if let Some(positional_ids) = &positional_ids {
            let positional_id = positional_ids.get(index)?;
            if candidate
                .id
                .as_deref()
                .is_some_and(|id| id != positional_id.as_str())
            {
                return None;
            }
            candidate.id.get_or_insert_with(|| positional_id.clone());
        }
        let id = candidate.id.as_ref()?;
        if !seen.insert(id.clone()) {
            return None;
        }
    }
    Some(())
}

fn proven_response_order(
    candidates: &[SegmentCandidate],
    initializer: Option<&Initializer>,
) -> Option<Vec<String>> {
    let initializer = initializer?;
    if initializer.loading_ids.is_empty() {
        return None;
    }

    let first_contains_initializer = candidates
        .first()
        .is_some_and(|candidate| candidate.contains_initializer);
    if first_contains_initializer {
        let loading_ids = initializer
            .loading_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut bootstrap_ids = initializer
            .graph
            .iter()
            .filter(|module| !loading_ids.contains(module.id.as_str()))
            .map(|module| module.id.clone());
        let bootstrap = bootstrap_ids.next();
        if bootstrap_ids.next().is_some() {
            return None;
        }
        if let Some(bootstrap) = bootstrap {
            return Some(
                std::iter::once(bootstrap)
                    .chain(initializer.loading_ids.iter().cloned())
                    .collect(),
            );
        }
    }

    Some(initializer.loading_ids.clone())
}

fn validate_boundary_helpers(
    candidates: &[SegmentCandidate],
    initializer: Option<&Initializer>,
) -> Option<()> {
    let mut begin_counts: HashMap<&str, usize> = HashMap::new();
    let mut end_counts: HashMap<&str, usize> = HashMap::new();
    let has_positional_basis = proven_response_order(candidates, initializer)
        .is_some_and(|ids| ids.len() == candidates.len());
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.marker.is_none())
    {
        match (
            candidate.begin_helper.as_deref(),
            candidate.end_helper.as_deref(),
        ) {
            (Some(begin), Some(end)) => {
                *begin_counts.entry(begin).or_default() += 1;
                *end_counts.entry(end).or_default() += 1;
            }
            (None, None) if has_positional_basis => {}
            _ => return None,
        }
    }

    // Markerless extraction relies on loader calls, so all such segments must
    // agree on the begin/end helper pair. An annotation remains authoritative
    // for synthetic modules that intentionally omit loader overhead.
    if begin_counts.len() > 1 || end_counts.len() > 1 {
        return None;
    }
    Some(())
}

fn statement_contains_initializer(stmt: &Stmt) -> bool {
    let mut collector = InitializerCollector::default();
    stmt.visit_with(&mut collector);
    !collector.invalid && collector.initializers.len() == 1
}

fn callee_export_name(call: &CallExpr) -> Option<Atom> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    expr_export_name(callee)
}

fn expr_export_name(expr: &Expr) -> Option<Atom> {
    match strip_parens(expr) {
        Expr::Ident(id) => Some(id.sym.clone()),
        Expr::Member(member) => member_prop_name(&member.prop),
        Expr::Seq(sequence) => sequence
            .exprs
            .last()
            .and_then(|expr| expr_export_name(expr)),
        _ => None,
    }
}

fn member_prop_name(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.clone()),
        MemberProp::Computed(computed) => match strip_parens(&computed.expr) {
            Expr::Lit(Lit::Str(Str { value, .. })) => Some(Atom::from(value.as_str()?)),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

fn call_target_key(call: &CallExpr) -> Option<String> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    expr_target_key(callee)
}

fn expr_target_key(expr: &Expr) -> Option<String> {
    match strip_parens(expr) {
        Expr::Ident(id) => Some(id.sym.to_string()),
        Expr::Member(member) => Some(format!(
            "{}.{}",
            expr_target_key(&member.obj)?,
            member_prop_name(&member.prop)?
        )),
        Expr::Call(call) if call.args.is_empty() => Some(format!("{}()", call_target_key(call)?)),
        Expr::Seq(sequence) => sequence.exprs.last().and_then(|expr| expr_target_key(expr)),
        _ => None,
    }
}

fn module_filename(id: &str, seen: &mut HashSet<String>) -> String {
    let sanitized = sanitize_relative_path(id, "module");
    let base =
        if sanitized.ends_with(".js") || sanitized.ends_with(".mjs") || sanitized.ends_with(".cjs")
        {
            sanitized
        } else {
            format!("{sanitized}.js")
        };
    deduplicate_filename(&base, seen)
}

fn deduplicate_filename(filename: &str, seen: &mut HashSet<String>) -> String {
    if seen.insert(filename.to_ascii_lowercase()) {
        return filename.to_string();
    }
    let (stem, extension) = filename
        .rsplit_once('.')
        .map_or((filename, "js"), |(stem, extension)| (stem, extension));
    let mut suffix = 2;
    loop {
        let candidate = format!("{stem}_{suffix}.{extension}");
        if seen.insert(candidate.to_ascii_lowercase()) {
            return candidate;
        }
        suffix += 1;
    }
}

struct EmittedSegment {
    code: String,
    source_map: Vec<GeneratedSourceMapPoint>,
}

fn emit_module(module: &Module, cm: Lrc<SourceMap>) -> anyhow::Result<EmittedSegment> {
    let mut output = Vec::new();
    let mut source_map = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default()
                .with_minify(false)
                .with_target(EsVersion::EsNext),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut output, Some(&mut source_map)),
        };
        emitter.emit_module(module)?;
    }
    let code = String::from_utf8(output)?;
    Ok(EmittedSegment {
        source_map: generated_source_map_points(&code, &cm, &source_map),
        code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base36_dependency_indexes() {
        let graph = decode_module_graph("base/a:0/b:0,1/c:2/d:0/e:0/f:0/g:0/h:0/i:0/j:0/k:a")
            .expect("valid graph");
        assert_eq!(graph[2].dependencies, ["base", "a"]);
        assert_eq!(graph[11].dependencies, ["j"]);
    }

    #[test]
    fn rejects_forward_and_malformed_dependency_indexes() {
        assert!(decode_module_graph("base/feature:2").is_none());
        assert!(decode_module_graph("base/feature:!").is_none());
        assert!(decode_module_graph("base/base:0").is_none());
    }

    #[test]
    fn accepts_empty_graph_used_by_single_module_responses() {
        assert_eq!(decode_module_graph(""), Some(Vec::new()));
    }

    #[test]
    fn marks_the_initializer_segment_as_entry() {
        let source = r#"
(function(shared) {
  try {
    shared._ModuleManager_initialize("base/feature:0", ["feature"]);
    shared.baseValue = 1;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
  try {
    shared.before("feature");
    shared.featureValue = 2;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
}).call(this, this.closureShared);
"#;
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::super::parse_es_module(source, "closure-entry.js", cm.clone())
            .expect("fixture should parse");
        let result = detect_from_module(&module, cm, source).expect("fixture should unpack");
        assert!(result
            .modules
            .iter()
            .any(|module| module.id == "base" && module.is_entry));
        assert!(result
            .modules
            .iter()
            .all(|module| module.id != "feature" || !module.is_entry));
    }
}
