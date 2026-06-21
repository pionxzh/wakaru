//! Multi-source merge preparation and numeric webpack module-ID rewriting.
//!
//! When unpacking multiple input files at once, extracted module filenames are
//! uniqued across inputs and numeric `require(<id>)` / async-chunk references
//! are rewritten to the merged output filenames when the target id is
//! unambiguous across all inputs.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rayon::prelude::*;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_core::ecma::ast::{
    CallExpr, Callee, Expr, ExprOrSpread, Lit, MemberExpr, MemberProp, Module, Str,
};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::super::io::{apply_fixer, parse_js, print_js};
use super::super::types::{UnpackOutput, UnpackWarning, UnpackWarningKind};
use crate::unpacker::UnpackedModule;
use crate::utils::paren::{strip_parens, strip_parens_mut};

pub(super) struct MultiSourceModule {
    module: UnpackedModule,
    allow_cross_chunk_rewrite: bool,
    allow_cycle_premerge: bool,
    chunk_ids: HashSet<usize>,
    input_filename: String,
    input_group: String,
}

impl MultiSourceModule {
    pub(super) fn detected(
        module: UnpackedModule,
        chunk_ids: HashSet<usize>,
        input_filename: String,
        allow_cycle_premerge: bool,
    ) -> Self {
        let input_group = input_group_for_filename(&input_filename);
        Self {
            module,
            allow_cross_chunk_rewrite: true,
            allow_cycle_premerge,
            chunk_ids,
            input_filename,
            input_group,
        }
    }

    pub(super) fn fallback(module: UnpackedModule) -> Self {
        Self {
            module,
            allow_cross_chunk_rewrite: false,
            allow_cycle_premerge: false,
            chunk_ids: HashSet::new(),
            input_filename: String::new(),
            input_group: String::new(),
        }
    }
}

pub(super) struct PreparedUnpackModule {
    pub(super) module: UnpackedModule,
    pub(super) numeric_rewrite: Option<NumericRewriteModuleContext>,
    pub(super) allow_cycle_premerge: bool,
}

impl PreparedUnpackModule {
    pub(super) fn plain(module: UnpackedModule) -> Self {
        Self {
            module,
            numeric_rewrite: None,
            allow_cycle_premerge: true,
        }
    }

    pub(super) fn with_cycle_premerge(module: UnpackedModule, allow_cycle_premerge: bool) -> Self {
        Self {
            module,
            numeric_rewrite: None,
            allow_cycle_premerge,
        }
    }
}

pub(super) struct NumericRewriteModuleContext {
    input_group: String,
    module_filename: String,
}

#[derive(Default)]
pub(super) struct NumericRewritePlan {
    plain_id_to_filename: HashMap<usize, String>,
    chunk_id_to_filename: HashMap<(String, usize, usize), String>,
}

impl NumericRewritePlan {
    pub(super) fn is_empty(&self) -> bool {
        self.plain_id_to_filename.is_empty() && self.chunk_id_to_filename.is_empty()
    }
}

pub(super) fn prepare_multi_source_modules(
    mut modules: Vec<MultiSourceModule>,
) -> (Vec<PreparedUnpackModule>, NumericRewritePlan) {
    assign_unique_module_filenames(&mut modules);
    let numeric_rewrite_plan = NumericRewritePlan {
        plain_id_to_filename: unique_numeric_module_id_map(&modules),
        chunk_id_to_filename: unique_numeric_chunk_module_id_map(&modules),
    };
    let has_rewrites = !numeric_rewrite_plan.is_empty();

    let modules = modules
        .into_iter()
        .map(|module| {
            let numeric_rewrite = if has_rewrites && module.allow_cross_chunk_rewrite {
                Some(NumericRewriteModuleContext {
                    input_group: module.input_group,
                    module_filename: module.module.filename.clone(),
                })
            } else {
                None
            };
            PreparedUnpackModule {
                module: module.module,
                numeric_rewrite,
                allow_cycle_premerge: module.allow_cycle_premerge,
            }
        })
        .collect();

    (modules, numeric_rewrite_plan)
}

fn assign_unique_module_filenames(modules: &mut [MultiSourceModule]) {
    let mut seen = HashSet::new();
    for module in modules {
        module.module.filename = deduplicate_module_filename(&module.module.filename, &mut seen);
    }
}

fn deduplicate_module_filename(filename: &str, seen: &mut HashSet<String>) -> String {
    let key = filename.to_lowercase();
    if seen.insert(key) {
        return filename.to_string();
    }

    let path = std::path::Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("js");
    let parent = path.parent().unwrap_or(std::path::Path::new(""));
    let mut n = 2u32;
    loop {
        let candidate = parent.join(format!("{stem}_{n}.{ext}"));
        let candidate = candidate.to_string_lossy().replace('\\', "/");
        let candidate_key = candidate.to_lowercase();
        if seen.insert(candidate_key) {
            return candidate;
        }
        n += 1;
    }
}

fn input_group_for_filename(filename: &str) -> String {
    let parent = std::path::Path::new(filename)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    normalize_input_group_path(parent)
}

fn normalize_input_group_path(path: &std::path::Path) -> String {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    normalize_path_lexically(&path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalize_path_lexically(path: &std::path::Path) -> std::path::PathBuf {
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn chunk_filename_matches_id(filename: &str, chunk_id: usize) -> bool {
    let Some(name) = std::path::Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    name == format!("{chunk_id}.js") || name == format!("{chunk_id}.bundle.js")
}

fn unique_numeric_module_id_map(modules: &[MultiSourceModule]) -> HashMap<usize, String> {
    let mut counts: HashMap<usize, (usize, String)> = HashMap::new();
    for module in modules {
        if !module.allow_cross_chunk_rewrite {
            continue;
        }
        let Ok(id) = module.module.id.parse::<usize>() else {
            continue;
        };
        let entry = counts
            .entry(id)
            .or_insert((0, module.module.filename.clone()));
        entry.0 += 1;
        entry.1 = module.module.filename.clone();
    }

    counts
        .into_iter()
        .filter_map(|(key, (count, filename))| (count == 1).then_some((key, filename)))
        .collect()
}

fn unique_numeric_chunk_module_id_map(
    modules: &[MultiSourceModule],
) -> HashMap<(String, usize, usize), String> {
    let mut counts: HashMap<(String, usize, usize), (usize, String)> = HashMap::new();
    for module in modules {
        if !module.allow_cross_chunk_rewrite || module.chunk_ids.is_empty() {
            continue;
        }
        let Ok(id) = module.module.id.parse::<usize>() else {
            continue;
        };
        for chunk_id in &module.chunk_ids {
            if !chunk_filename_matches_id(&module.input_filename, *chunk_id) {
                continue;
            }
            let entry = counts
                .entry((module.input_group.clone(), *chunk_id, id))
                .or_insert((0, module.module.filename.clone()));
            entry.0 += 1;
            entry.1 = module.module.filename.clone();
        }
    }

    counts
        .into_iter()
        .filter_map(|(key, (count, filename))| (count == 1).then_some((key, filename)))
        .collect()
}

pub(super) fn apply_numeric_rewrites(
    module: &mut Module,
    unresolved_mark: Mark,
    context: Option<&NumericRewriteModuleContext>,
    plan: &NumericRewritePlan,
) {
    let Some(context) = context else {
        return;
    };
    if plan.is_empty() {
        return;
    }

    module.visit_mut_with(&mut WebpackNumericReferenceRewriter {
        input_group: &context.input_group,
        module_filename: &context.module_filename,
        unresolved_mark,
        plain_id_to_filename: &plan.plain_id_to_filename,
        chunk_id_to_filename: &plan.chunk_id_to_filename,
    });
}

pub(super) fn emit_raw_modules_with_numeric_rewrites(
    modules: Vec<PreparedUnpackModule>,
    numeric_rewrite_plan: NumericRewritePlan,
) -> Result<UnpackOutput> {
    if numeric_rewrite_plan.is_empty() {
        return Ok(UnpackOutput {
            modules: modules
                .into_iter()
                .map(|module| (module.module.filename, module.module.code))
                .collect(),
            warnings: Vec::new(),
            detected_formats: Vec::new(),
        });
    }

    let triples = modules
        .into_par_iter()
        .map(|unpacked| {
            match GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                let mut module =
                    parse_js(&unpacked.module.code, &unpacked.module.filename, cm.clone())?;
                let unresolved_mark = Mark::new();
                let top_level_mark = Mark::new();
                module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
                apply_numeric_rewrites(
                    &mut module,
                    unresolved_mark,
                    unpacked.numeric_rewrite.as_ref(),
                    &numeric_rewrite_plan,
                );
                apply_fixer(&mut module)?;
                print_js(&module, cm)
            }) {
                Ok(code) => (unpacked.module.filename, code, None),
                Err(e) => {
                    let warning = UnpackWarning::new(
                        unpacked.module.filename.clone(),
                        UnpackWarningKind::RawNormalizationFailed,
                        format!("raw numeric rewrite failed, preserving unparsed code: {e}"),
                    );
                    (
                        unpacked.module.filename,
                        unpacked.module.code,
                        Some(warning),
                    )
                }
            }
        })
        .collect::<Vec<_>>();

    let mut modules = Vec::new();
    let mut warnings = Vec::new();
    for (filename, code, warning) in triples {
        modules.push((filename, code));
        if let Some(warning) = warning {
            warnings.push(warning);
        }
    }

    Ok(UnpackOutput {
        modules,
        warnings,
        detected_formats: Vec::new(),
    })
}

struct WebpackNumericReferenceRewriter<'a> {
    input_group: &'a str,
    module_filename: &'a str,
    unresolved_mark: Mark,
    plain_id_to_filename: &'a HashMap<usize, String>,
    chunk_id_to_filename: &'a HashMap<(String, usize, usize), String>,
}

impl VisitMut for WebpackNumericReferenceRewriter<'_> {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        self.rewrite_async_chunk_t_bind(call);
        call.visit_mut_children_with(self);
        self.rewrite_plain_require(call);
    }
}

impl WebpackNumericReferenceRewriter<'_> {
    fn rewrite_plain_require(&self, call: &mut CallExpr) {
        let Callee::Expr(callee_expr) = &call.callee else {
            return;
        };
        let Expr::Ident(callee) = strip_parens(callee_expr) else {
            return;
        };
        if callee.sym.as_ref() != "require" || callee.ctxt.outer() != self.unresolved_mark {
            return;
        }

        let Some(module_id) = numeric_single_arg_id(call) else {
            return;
        };
        let Some(filename) = self.plain_id_to_filename.get(&module_id) else {
            return;
        };
        rewrite_numeric_arg_to_filename(&mut call.args[0], self.module_filename, filename);
    }

    fn rewrite_async_chunk_t_bind(&self, call: &mut CallExpr) {
        let Some((runtime, chunk_id)) = self.extract_then_chunk_loader(&call.callee) else {
            return;
        };
        let Some(arg) = call.args.first_mut() else {
            return;
        };
        if arg.spread.is_some() {
            return;
        }
        let Expr::Call(bind_call) = strip_parens_mut(&mut arg.expr) else {
            return;
        };
        self.rewrite_t_bind_module_arg(bind_call, &runtime, chunk_id);
    }

    fn extract_then_chunk_loader(&self, callee: &Callee) -> Option<(RuntimeIdent, usize)> {
        let Callee::Expr(callee_expr) = callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee_expr) else {
            return None;
        };
        if !member_prop_is(prop, "then") {
            return None;
        }

        let Expr::Call(load_call) = strip_parens(obj) else {
            return None;
        };
        self.extract_runtime_member_numeric_arg(load_call, "e", 0)
    }

    fn rewrite_t_bind_module_arg(
        &self,
        call: &mut CallExpr,
        expected_runtime: &RuntimeIdent,
        chunk_id: usize,
    ) {
        let Callee::Expr(callee_expr) = &call.callee else {
            return;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee_expr) else {
            return;
        };
        if !member_prop_is(prop, "bind") {
            return;
        }
        let Some(runtime) = self.extract_runtime_t_member(obj) else {
            return;
        };
        if &runtime != expected_runtime {
            return;
        }

        let Some(this_arg) = call.args.first() else {
            return;
        };
        if this_arg.spread.is_some() {
            return;
        }
        let Expr::Ident(this_ident) = strip_parens(&this_arg.expr) else {
            return;
        };
        if runtime != RuntimeIdent::from_ident(this_ident) {
            return;
        }

        self.rewrite_chunk_module_arg(&mut call.args, 1, chunk_id);
    }

    fn extract_runtime_member_numeric_arg(
        &self,
        call: &CallExpr,
        expected_prop: &str,
        arg_index: usize,
    ) -> Option<(RuntimeIdent, usize)> {
        let Callee::Expr(callee_expr) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee_expr) else {
            return None;
        };
        if !member_prop_is(prop, expected_prop) {
            return None;
        }
        let Expr::Ident(runtime) = strip_parens(obj) else {
            return None;
        };
        let arg = call.args.get(arg_index)?;
        if arg.spread.is_some() {
            return None;
        }
        let module_id = numeric_arg_id(&arg.expr)?;
        Some((RuntimeIdent::from_ident(runtime), module_id))
    }

    fn extract_runtime_t_member(&self, expr: &Expr) -> Option<RuntimeIdent> {
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(expr) else {
            return None;
        };
        if !member_prop_is(prop, "t") {
            return None;
        }
        let Expr::Ident(runtime) = strip_parens(obj) else {
            return None;
        };
        Some(RuntimeIdent::from_ident(runtime))
    }

    fn rewrite_chunk_module_arg(&self, args: &mut [ExprOrSpread], index: usize, chunk_id: usize) {
        let Some(arg) = args.get_mut(index) else {
            return;
        };
        if arg.spread.is_some() {
            return;
        }
        let Some(module_id) = numeric_arg_id(&arg.expr) else {
            return;
        };
        let Some(filename) =
            self.chunk_id_to_filename
                .get(&(self.input_group.to_string(), chunk_id, module_id))
        else {
            return;
        };
        rewrite_numeric_arg_to_filename(arg, self.module_filename, filename);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RuntimeIdent {
    sym: Atom,
    ctxt: SyntaxContext,
}

impl RuntimeIdent {
    fn from_ident(ident: &swc_core::ecma::ast::Ident) -> Self {
        Self {
            sym: ident.sym.clone(),
            ctxt: ident.ctxt,
        }
    }
}

fn numeric_arg_id(expr: &Expr) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = strip_parens(expr) else {
        return None;
    };
    let value = number.value;
    if value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    Some(value as usize)
}

fn numeric_single_arg_id(call: &CallExpr) -> Option<usize> {
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    numeric_arg_id(&call.args[0].expr)
}

fn rewrite_numeric_arg_to_filename(arg: &mut ExprOrSpread, from_filename: &str, filename: &str) {
    let path = relative_import_specifier(from_filename, filename);
    *arg.expr = Expr::Lit(Lit::Str(Str {
        span: DUMMY_SP,
        value: path.into(),
        raw: None,
    }));
}

/// Resolve a relative module specifier (`./x`, `../y/z.js`) written in
/// `from_filename` to the normalized module key it points at. Returns `None`
/// for bare/package specifiers (`react`, `fs`) that do not name a local module.
pub(super) fn resolve_relative_specifier(from_filename: &str, spec: &str) -> Option<String> {
    if !(spec.starts_with("./") || spec.starts_with("../")) {
        return None;
    }
    let from = from_filename.replace('\\', "/");
    let mut parts: Vec<&str> = from
        .rsplit_once('/')
        .map(|(dir, _)| dir.split('/').filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

pub(super) fn relative_import_specifier(from_filename: &str, target_filename: &str) -> String {
    let from = from_filename.replace('\\', "/");
    let target = target_filename.replace('\\', "/");
    let from_dir: Vec<&str> = from
        .rsplit_once('/')
        .map(|(dir, _)| dir.split('/').filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    let target_parts: Vec<&str> = target.split('/').filter(|part| !part.is_empty()).collect();

    let mut common = 0usize;
    while common < from_dir.len()
        && common < target_parts.len()
        && from_dir[common] == target_parts[common]
    {
        common += 1;
    }

    let mut parts = Vec::new();
    parts.extend(std::iter::repeat_n("..", from_dir.len() - common));
    parts.extend(target_parts[common..].iter().copied());

    let path = parts.join("/");
    if path.starts_with("../") {
        path
    } else {
        format!("./{path}")
    }
}

fn member_prop_is(prop: &MemberProp, expected: &str) -> bool {
    matches!(prop, MemberProp::Ident(ident) if ident.sym.as_ref() == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_rewrite_paths_are_relative_to_nested_module() {
        assert_eq!(
            relative_import_specifier("module-200.js", "module-100.js"),
            "./module-100.js"
        );
        assert_eq!(
            relative_import_specifier("module-11111.js", "module-11111/chunk_value.js"),
            "./module-11111/chunk_value.js"
        );
        assert_eq!(
            relative_import_specifier("module-22222/chunk_value.js", "module-44444.js"),
            "../module-44444.js"
        );
        assert_eq!(
            relative_import_specifier("module-22222/chunk_value.js", "module-22222/chunk_other.js"),
            "./chunk_other.js"
        );
        assert_eq!(
            relative_import_specifier("module-22222/chunk_value.js", "module-33333/chunk_extra.js"),
            "../module-33333/chunk_extra.js"
        );
    }

    #[test]
    fn numeric_rewrite_plan_applies_to_existing_ast_without_source_stabilization() {
        let modules = vec![
            MultiSourceModule::detected(
                UnpackedModule {
                    id: "20".to_string(),
                    is_entry: false,
                    code: "const other = require(999);".to_string(),
                    filename: "module-20.js".to_string(),
                },
                HashSet::new(),
                "entry.js".to_string(),
                true,
            ),
            MultiSourceModule::detected(
                UnpackedModule {
                    id: "999".to_string(),
                    is_entry: false,
                    code: "export default 1;".to_string(),
                    filename: "module-999.js".to_string(),
                },
                HashSet::new(),
                "chunk.js".to_string(),
                true,
            ),
        ];

        let (prepared, plan) = prepare_multi_source_modules(modules);
        assert!(
            prepared[0].module.code.contains("require(999)"),
            "prepare should keep source strings untouched"
        );

        let output = GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_js(
                &prepared[0].module.code,
                &prepared[0].module.filename,
                cm.clone(),
            )
            .expect("fixture should parse");
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
            apply_numeric_rewrites(
                &mut module,
                unresolved_mark,
                prepared[0].numeric_rewrite.as_ref(),
                &plan,
            );
            apply_fixer(&mut module).expect("fixer should not panic on fixture");
            print_js(&module, cm).expect("fixture should print")
        });

        assert!(
            output.contains(r#"require("./module-999.js")"#),
            "rewrite plan should apply to the already-parsed AST:\n{output}"
        );
    }
}
