use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, Span, GLOBALS};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, CallExpr, Callee, Expr, ExprOrSpread, ExprStmt,
    FnExpr, Ident, IdentName, Lit, MemberExpr, MemberProp, Module, ModuleItem, Number, Pat,
    PropName, SimpleAssignTarget, Stmt, Str, UnaryOp,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::rules::apply_default_rules;
use crate::unpacker::{UnpackResult, UnpackedModule};

/// Renames param identifiers in a function body to standard names.
/// This visitor avoids renaming property keys in MemberExpr and object property names.
/// Only identifiers that are unresolved free-variable references (marked with
/// `unresolved_mark` by `resolver()`) are renamed, so inner-scope bindings that
/// happen to share the same symbol are left untouched.
struct ParamRenamer {
    renames: Vec<(Atom, Atom)>,
    unresolved_mark: Mark,
}

impl VisitMut for ParamRenamer {
    fn visit_mut_ident(&mut self, id: &mut Ident) {
        // Only rename identifiers that resolver() marked as free/unresolved references.
        // Bound identifiers (inner function params, local `var`s, etc.) carry a different
        // SyntaxContext and must not be touched.
        if id.ctxt.outer() != self.unresolved_mark {
            return;
        }
        for (old, new) in &self.renames {
            if &id.sym == old {
                id.sym = new.clone();
                break;
            }
        }
    }

    // Don't rename computed property keys — only visit the expression inside
    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_mut_with(self);
        }
        // Static member props (Ident/PrivateName) are NOT identifiers referencing variables
        // so we must not rename them.
    }

    // Don't rename object literal property names
    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}
}

/// Removes `require.r(exports)` calls and converts `require.d(exports, "name", getter)` to
/// `exports.name = val`. This normalizes webpack runtime helpers before applying rules.
struct WebpackRuntimeNormalizer {
    /// The symbol name used for the require-like parameter
    require_sym: Atom,
    /// The symbol name used for the exports parameter
    exports_sym: Atom,
    /// Only match identifiers that resolver() marked as unresolved free-variable references.
    unresolved_mark: Mark,
}

impl VisitMut for WebpackRuntimeNormalizer {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let mut new_items: Vec<ModuleItem> = Vec::with_capacity(items.len());
        for item in items.drain(..) {
            if let ModuleItem::Stmt(stmt) = item {
                let expanded = self.expand_stmt(stmt);
                for s in expanded {
                    if let Some(replacement) = self.try_convert_stmt(&s) {
                        new_items.extend(replacement.into_iter().map(ModuleItem::Stmt));
                    } else {
                        new_items.push(ModuleItem::Stmt(s));
                    }
                }
            } else {
                new_items.push(item);
            }
        }
        *items = new_items;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        let mut new_stmts: Vec<Stmt> = Vec::with_capacity(stmts.len());
        for stmt in stmts.drain(..) {
            // First, expand sequence expressions into individual statements so that
            // `n.r(t), n.d(t, "x", fn)` in a single ExprStmt is split before matching.
            let expanded = self.expand_stmt(stmt);
            for s in expanded {
                if let Some(replacement) = self.try_convert_stmt(&s) {
                    new_stmts.extend(replacement);
                } else {
                    new_stmts.push(s);
                }
            }
        }
        *stmts = new_stmts;
    }
}

impl WebpackRuntimeNormalizer {
    /// Expand a sequence ExprStmt into individual ExprStmts.
    fn expand_stmt(&self, stmt: Stmt) -> Vec<Stmt> {
        if let Stmt::Expr(ExprStmt { expr, span }) = &stmt {
            if let Expr::Seq(seq) = &**expr {
                return seq.exprs.iter().map(|e| {
                    Stmt::Expr(ExprStmt {
                        span: *span,
                        expr: e.clone(),
                    })
                }).collect();
            }
        }
        vec![stmt]
    }

    /// Returns None to keep the statement as-is, or Some(vec) to replace it (possibly empty to remove).
    fn try_convert_stmt(&self, stmt: &Stmt) -> Option<Vec<Stmt>> {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            return None;
        };
        let Expr::Call(call) = &**expr else {
            return None;
        };

        // Check if the callee is `<require>.r` or `<require>.d`
        let Callee::Expr(callee_expr) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
            return None;
        };

        let Expr::Ident(callee_obj) = &**obj else {
            return None;
        };

        if callee_obj.sym != self.require_sym || callee_obj.ctxt.outer() != self.unresolved_mark {
            return None;
        }

        let MemberProp::Ident(prop_name) = prop else {
            return None;
        };

        match prop_name.sym.as_ref() {
            "r" => {
                // Remove `require.r(exports)` entirely
                Some(vec![])
            }
            "d" => {
                // Convert `require.d(exports, "name", function() { return val; })` to `exports.name = val;`
                if call.args.len() != 3 {
                    return None;
                }
                let name_arg = &call.args[1];
                let getter_arg = &call.args[2];

                // Second arg must be a string literal (the export name)
                let Expr::Lit(Lit::Str(name_str)) = &*name_arg.expr else {
                    return None;
                };
                // name_str.value is Wtf8Atom; convert to Atom via &str
                let export_name: Atom = name_str.value.as_str()
                    .map(Atom::from)
                    .unwrap_or_else(|| Atom::from(name_str.value.to_string_lossy().into_owned().as_str()));

                // Third arg must be a getter function: function() { return val; } or () => val
                let val_expr = extract_getter_value(&getter_arg.expr)?;

                // Build: exports.name = val;
                let span = stmt.span();
                let exports_ident = Ident::new(self.exports_sym.clone(), span, Default::default());
                let assign_stmt = build_member_assign(exports_ident, export_name, val_expr, span);
                Some(vec![assign_stmt])
            }
            _ => None,
        }
    }
}

/// Rewrites `require(N)` calls (where N is a numeric literal) to `require("./filename.js")`.
/// This lets un-esm convert them to proper ES import statements.
struct RequireIdRewriter<'a> {
    require_sym: Atom,
    unresolved_mark: Mark,
    id_to_filename: &'a std::collections::HashMap<usize, String>,
}

impl VisitMut for RequireIdRewriter<'_> {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        // Recurse first
        call.visit_mut_children_with(self);

        // Match: require(N) where callee is the require ident
        let Callee::Expr(callee_expr) = &call.callee else {
            return;
        };
        let Expr::Ident(callee_ident) = &**callee_expr else {
            return;
        };
        if callee_ident.sym != self.require_sym || callee_ident.ctxt.outer() != self.unresolved_mark {
            return;
        }
        if call.args.len() != 1 || call.args[0].spread.is_some() {
            return;
        }
        let Expr::Lit(Lit::Num(Number { value, .. })) = &*call.args[0].expr else {
            return;
        };

        let id = *value as usize;
        // Only rewrite if value is a non-negative integer
        if (*value) < 0.0 || (*value).fract() != 0.0 {
            return;
        }

        if let Some(filename) = self.id_to_filename.get(&id) {
            let path = format!("./{filename}");
            call.args[0].expr = Box::new(Expr::Lit(Lit::Str(Str {
                span: Default::default(),
                value: path.as_str().into(),
                raw: None,
            })));
        }
    }
}

/// Rewrites `require.n(expr)` to `() => expr`.
/// webpack's `__webpack_require__.n` wraps a module in a default-export getter.
/// After ESM conversion, `require.n(r)` is equivalent to `() => r`.
/// The call sites `o()` are later simplified by UnIife's expression-body IIFE handling.
struct RequireNRewriter {
    require_sym: Atom,
    unresolved_mark: Mark,
}

impl VisitMut for RequireNRewriter {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        // Match: require.n(single_arg)
        let Expr::Call(call) = expr else { return };
        if call.args.len() != 1 || call.args[0].spread.is_some() {
            return;
        }
        let Callee::Expr(callee_expr) = &call.callee else { return };
        let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else { return };
        let Expr::Ident(obj_ident) = &**obj else { return };
        if obj_ident.sym != self.require_sym || obj_ident.ctxt.outer() != self.unresolved_mark {
            return;
        }
        let MemberProp::Ident(prop_ident) = prop else { return };
        if prop_ident.sym.as_ref() != "n" {
            return;
        }

        // Replace `require.n(arg)` with `() => arg`
        let arg = call.args[0].expr.clone();
        *expr = Expr::Arrow(swc_core::ecma::ast::ArrowExpr {
            span: Default::default(),
            ctxt: Default::default(),
            params: vec![],
            body: Box::new(swc_core::ecma::ast::BlockStmtOrExpr::Expr(arg)),
            is_async: false,
            is_generator: false,
            type_params: None,
            return_type: None,
        });
    }
}

/// Extracts the return value from a getter function `function() { return val; }` or `() => val`.
fn extract_getter_value(expr: &Expr) -> Option<Box<Expr>> {
    match expr {
        Expr::Fn(fn_expr) => {
            let body = fn_expr.function.body.as_ref()?;
            // Must have exactly one statement: return <val>
            if body.stmts.len() == 1 {
                if let Stmt::Return(ret) = &body.stmts[0] {
                    return ret.arg.clone();
                }
            }
            None
        }
        Expr::Arrow(arrow_expr) => {
            use swc_core::ecma::ast::BlockStmtOrExpr;
            match &*arrow_expr.body {
                BlockStmtOrExpr::Expr(e) => Some(e.clone()),
                BlockStmtOrExpr::BlockStmt(block) => {
                    if block.stmts.len() == 1 {
                        if let Stmt::Return(ret) = &block.stmts[0] {
                            return ret.arg.clone();
                        }
                    }
                    None
                }
            }
        }
        _ => None,
    }
}

/// Builds `obj.name = val` as a Stmt.
fn build_member_assign(obj_ident: Ident, prop_name: Atom, val: Box<Expr>, span: Span) -> Stmt {
    use swc_core::ecma::ast::{AssignExpr, AssignOp, AssignTarget, MemberExpr, SimpleAssignTarget};

    let member = MemberExpr {
        span,
        obj: Box::new(Expr::Ident(obj_ident)),
        prop: MemberProp::Ident(IdentName::new(prop_name, span)),
    };

    let assign = AssignExpr {
        span,
        op: AssignOp::Assign,
        left: AssignTarget::Simple(SimpleAssignTarget::Member(member)),
        right: val,
    };

    Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(assign)),
    })
}

/// Detects whether the parsed module is a webpack4 bundle and extracts modules,
/// skipping `apply_default_rules`. Returns the intermediate state after webpack
/// normalization (param renaming, require.d / require.r conversion, require(N)
/// rewriting, require.n rewriting) but before SimplifySequence, UnEsm, etc.
pub fn detect_and_extract_raw(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = parse_es_module(source, cm.clone()).ok()?;

        for item in &module.body {
            let ModuleItem::Stmt(stmt) = item else {
                continue;
            };
            if let Some(result) = try_extract_from_stmt_raw(stmt, cm.clone()) {
                return Some(result);
            }
        }
        None
    })
}

/// Detects whether the parsed module is a webpack4 bundle and extracts modules.
pub fn detect_and_extract(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = parse_es_module(source, cm.clone()).ok()?;

        // Find the webpack IIFE call in the top-level statements
        for item in &module.body {
            let ModuleItem::Stmt(stmt) = item else {
                continue;
            };
            if let Some(result) = try_extract_from_stmt(stmt, cm.clone()) {
                return Some(result);
            }
        }
        None
    })
}

/// Try to extract from a top-level statement that might be a webpack4 IIFE (raw, no default rules).
fn try_extract_from_stmt_raw(stmt: &Stmt, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
    let call = match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => match &**expr {
            Expr::Unary(u) if u.op == UnaryOp::Bang => {
                extract_call_from_expr(&u.arg)?
            }
            other => extract_call_from_expr(other)?,
        },
        _ => return None,
    };

    extract_webpack4_modules(call, cm, false)
}

/// Try to extract from a top-level statement that might be a webpack4 IIFE.
fn try_extract_from_stmt(stmt: &Stmt, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
    let call = match stmt {
        // `!function(...){...}([...])` — UnaryExpr with !
        Stmt::Expr(ExprStmt { expr, .. }) => match &**expr {
            Expr::Unary(u) if u.op == UnaryOp::Bang => {
                extract_call_from_expr(&u.arg)?
            }
            other => extract_call_from_expr(other)?,
        },
        _ => return None,
    };

    extract_webpack4_modules(call, cm, true)
}

fn extract_call_from_expr(expr: &Expr) -> Option<&CallExpr> {
    match expr {
        Expr::Call(call) => Some(call),
        _ => None,
    }
}

/// Given a CallExpr that should be `bootstrapFn([module0, module1, ...])`, extract the modules.
/// When `apply_rules` is false, `apply_default_rules` is skipped (raw output).
fn extract_webpack4_modules(call: &CallExpr, cm: Lrc<SourceMap>, apply_rules: bool) -> Option<UnpackResult> {
    // Callee must be a FnExpr (the bootstrap function)
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let Expr::Fn(bootstrap_fn) = &**callee_expr else {
        return None;
    };

    // Must have exactly one argument: an ArrayLit
    if call.args.len() != 1 {
        return None;
    }
    let array_lit = match &*call.args[0].expr {
        Expr::Array(a) => a,
        _ => return None,
    };

    // Array must have at least one element
    if array_lit.elems.is_empty() {
        return None;
    }

    // Each element should be a FnExpr (or null for holes)
    let module_fns: Vec<Option<&FnExpr>> = array_lit.elems.iter().map(|elem| {
        match elem {
            Some(ExprOrSpread { expr, .. }) => {
                if let Expr::Fn(fn_expr) = &**expr {
                    Some(fn_expr)
                } else {
                    None
                }
            }
            None => None, // array hole
        }
    }).collect();

    // Validate: at least one function with at least 1 param
    let has_module_fn = module_fns.iter().any(|f| {
        f.map(|fn_expr| !fn_expr.function.params.is_empty()).unwrap_or(false)
    });
    if !has_module_fn {
        return None;
    }

    // Find entry module IDs by scanning the bootstrap function body
    let entry_ids = find_entry_ids(bootstrap_fn);

    // Extract require-fn symbol from bootstrap fn body (to know which param is require-like)
    // In webpack4, the bootstrap fn typically has a single param `e` (the modules array)
    // and an inner function `n` (the require). We detect `n.s = N` to find entry.
    // We don't need the symbol since we're already looking at module functions.

    // Build a map from module index → filename so require(N) can be rewritten
    // to require("./module-N.js") / require("./entry.js") etc.
    let id_to_filename: std::collections::HashMap<usize, String> = {
        let total = module_fns.len();
        (0..total)
            .filter_map(|i| {
                module_fns.get(i)?.as_ref()?;
                let name = if entry_ids.contains(&i) {
                    if entry_ids.len() == 1 {
                        "entry.js".to_string()
                    } else {
                        format!("entry-{i}.js")
                    }
                } else {
                    format!("module-{i}.js")
                };
                Some((i, name))
            })
            .collect()
    };

    let mut modules = Vec::new();

    for (idx, maybe_fn) in module_fns.iter().enumerate() {
        let Some(fn_expr) = maybe_fn else {
            // Array hole — skip
            continue;
        };

        let is_entry = entry_ids.contains(&idx);

        // Extract param names (up to 3: module, exports, require)
        let params = &fn_expr.function.params;
        let param_syms: Vec<Atom> = params.iter().filter_map(|p| {
            if let Pat::Ident(bi) = &p.pat {
                Some(bi.sym.clone())
            } else {
                None
            }
        }).collect();

        // Build renaming map: param[0] -> "module", param[1] -> "exports", param[2] -> "require"
        let standard_names = ["module", "exports", "require"];
        let renames: Vec<(Atom, Atom)> = param_syms.iter().enumerate()
            .filter_map(|(i, sym)| {
                let target = *standard_names.get(i)?;
                if sym.as_ref() == target {
                    None // already correct
                } else {
                    Some((sym.clone(), Atom::from(target)))
                }
            })
            .collect();

        // Get the module's body statements
        let body_stmts = match &fn_expr.function.body {
            Some(body) => body.stmts.clone(),
            None => vec![],
        };

        // Determine the (possibly renamed) exports/require symbols for normalizer
        let exports_sym = {
            let orig = param_syms.get(1).cloned().unwrap_or_else(|| Atom::from("exports"));
            // After renaming, it will be "exports" if there's a rename, else whatever it was
            if renames.iter().any(|(old, _)| old == &orig) {
                Atom::from("exports")
            } else {
                orig
            }
        };
        let require_sym = {
            let orig = param_syms.get(2).cloned().unwrap_or_else(|| Atom::from("require"));
            if renames.iter().any(|(old, _)| old == &orig) {
                Atom::from("require")
            } else {
                orig
            }
        };

        // Build a synthetic Module wrapping the body statements
        let mut synthetic_module = build_module_from_stmts(body_stmts);

        // Step 0: run resolver() first so every identifier gets a unique SyntaxContext.
        // Unresolved free-variable references (the factory params like `e`, `t`, `n`)
        // receive `unresolved_mark` as their outer mark; bound identifiers (inner params,
        // locals) get a different mark — allowing ParamRenamer to skip them safely.
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        synthetic_module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        // Step 1: rename params to standard names (scope-aware via unresolved_mark)
        if !renames.is_empty() {
            let mut renamer = ParamRenamer { renames, unresolved_mark };
            synthetic_module.visit_mut_with(&mut renamer);
        }

        // Step 1b: rewrite require(N) → require("./module-N.js") so un-esm can convert them
        // After step 1, the require parameter is now named "require"
        let post_rename_require_sym = if param_syms.get(2).map(|s| s.as_ref()) != Some("require") {
            Atom::from("require")
        } else {
            param_syms.get(2).cloned().unwrap_or_else(|| Atom::from("require"))
        };
        {
            let mut id_rewriter = RequireIdRewriter {
                require_sym: post_rename_require_sym.clone(),
                unresolved_mark,
                id_to_filename: &id_to_filename,
            };
            synthetic_module.visit_mut_with(&mut id_rewriter);
        }

        // Step 1c: rewrite require.n(expr) → () => expr (webpack default-export getter)
        {
            let mut n_rewriter = RequireNRewriter {
                require_sym: post_rename_require_sym.clone(),
                unresolved_mark,
            };
            synthetic_module.visit_mut_with(&mut n_rewriter);
        }

        // Step 2: normalize webpack runtime helpers (require.r / require.d)
        // After renaming, require_sym may still be the original if it wasn't renamed
        // Use the post-rename name (always "require" if renamed, else original)
        let final_require_sym = if param_syms.get(2).map(|s| s.as_ref()) == Some("require") {
            Atom::from("require")
        } else {
            // It was renamed (or doesn't exist)
            require_sym.clone()
        };
        let mut normalizer = WebpackRuntimeNormalizer {
            require_sym: final_require_sym,
            exports_sym,
            unresolved_mark,
        };
        synthetic_module.visit_mut_with(&mut normalizer);

        // Step 3: optionally apply default rules
        if apply_rules {
            apply_default_rules(&mut synthetic_module);
        }
        synthetic_module.visit_mut_with(&mut fixer(None));

        // Step 4: emit to code
        let code = match emit_module(&synthetic_module, cm.clone()) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let filename = if is_entry {
            if entry_ids.len() == 1 {
                "entry.js".to_string()
            } else {
                format!("entry-{idx}.js")
            }
        } else {
            format!("module-{idx}.js")
        };

        modules.push(UnpackedModule {
            id: idx,
            is_entry,
            code,
            filename,
        });
    }

    if modules.is_empty() {
        return None;
    }

    Some(UnpackResult { modules })
}

/// Scan the bootstrap function body for `n.s = <number>` or `n(n.s = <number>)` patterns
/// to identify the entry module ID(s).
fn find_entry_ids(bootstrap_fn: &FnExpr) -> Vec<usize> {
    let body = match &bootstrap_fn.function.body {
        Some(b) => b,
        None => return vec![],
    };

    let mut entries = Vec::new();
    for stmt in &body.stmts {
        collect_entry_ids_from_stmt(stmt, &mut entries);
    }
    entries
}

fn collect_entry_ids_from_stmt(stmt: &Stmt, entries: &mut Vec<usize>) {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return;
    };
    collect_entry_ids_from_expr(expr, entries);
}

fn collect_entry_ids_from_expr(expr: &Expr, entries: &mut Vec<usize>) {
    match expr {
        // `n(n.s = 51)` — call where arg is assignment to <fn>.s
        Expr::Call(call) => {
            for arg in &call.args {
                if let Some(id) = extract_entry_id_from_expr(&arg.expr) {
                    entries.push(id);
                }
                collect_entry_ids_from_expr(&arg.expr, entries);
            }
        }
        // `n.s = 51` at statement level
        Expr::Assign(assign) => {
            if let Some(id) = extract_entry_id_from_assign(assign) {
                entries.push(id);
            }
        }
        // Sequences like `n.m=e, n.c=t, ..., n(n.s=51)`
        Expr::Seq(seq) => {
            for e in &seq.exprs {
                collect_entry_ids_from_expr(e, entries);
            }
        }
        _ => {}
    }
}

fn extract_entry_id_from_expr(expr: &Expr) -> Option<usize> {
    if let Expr::Assign(assign) = expr {
        return extract_entry_id_from_assign(assign);
    }
    None
}

fn extract_entry_id_from_assign(assign: &AssignExpr) -> Option<usize> {
    if assign.op != AssignOp::Assign {
        return None;
    }
    // Left must be MemberExpr with prop "s"
    let AssignTarget::Simple(SimpleAssignTarget::Member(m)) = &assign.left else {
        return None;
    };
    let MemberProp::Ident(prop) = &m.prop else {
        return None;
    };
    if prop.sym.as_ref() != "s" {
        return None;
    }
    // Right must be a numeric literal
    if let Expr::Lit(Lit::Num(n)) = &*assign.right {
        let id = n.value as usize;
        return Some(id);
    }
    None
}

/// Build a synthetic Module from a list of statements.
fn build_module_from_stmts(stmts: Vec<Stmt>) -> Module {
    use swc_core::ecma::ast::Module;
    Module {
        span: Default::default(),
        body: stmts.into_iter().map(ModuleItem::Stmt).collect(),
        shebang: None,
    }
}

fn parse_es_module(source: &str, cm: Lrc<SourceMap>) -> anyhow::Result<Module> {
    use anyhow::anyhow;
    let fm = cm.new_source_file(
        FileName::Custom("webpack4.js".to_string()).into(),
        source.to_string(),
    );
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax::default()),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    parser
        .parse_module()
        .map_err(|e| anyhow!("parse error: {e:?}"))
}

fn emit_module(module: &Module, cm: Lrc<SourceMap>) -> anyhow::Result<String> {
    use anyhow::anyhow;
    let mut output = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut output, None),
        };
        emitter
            .emit_module(module)
            .map_err(|e| anyhow!("emit error: {e:?}"))?;
    }
    String::from_utf8(output).map_err(|e| anyhow!("utf8 error: {e}"))
}

// Need Stmt::span() helper — import it
trait StmtSpan {
    fn span(&self) -> Span;
}

impl StmtSpan for Stmt {
    fn span(&self) -> Span {
        use swc_core::ecma::ast::*;
        match self {
            Stmt::Expr(e) => e.span,
            Stmt::Block(b) => b.span,
            Stmt::Return(r) => r.span,
            Stmt::If(i) => i.span,
            Stmt::Throw(t) => t.span,
            Stmt::Decl(d) => match d {
                Decl::Var(v) => v.span,
                Decl::Fn(f) => f.function.span,
                Decl::Class(c) => c.class.span,
                _ => Default::default(),
            },
            _ => Default::default(),
        }
    }
}
