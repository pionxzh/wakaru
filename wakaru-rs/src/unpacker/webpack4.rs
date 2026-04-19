use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, Span, GLOBALS};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BlockStmt, BlockStmtOrExpr,
    CallExpr, Callee, CondExpr, Expr, ExprOrSpread, ExprStmt, FnExpr, Id, Ident, IdentName, Lit,
    MemberExpr, MemberProp, Module, ModuleItem, Number, Pat, SimpleAssignTarget, Stmt, Str,
    UnaryExpr, UnaryOp, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::utils::replace_ident;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::rules::apply_default_rules;
use crate::unpacker::{UnpackResult, UnpackedModule};

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
                return seq
                    .exprs
                    .iter()
                    .map(|e| {
                        Stmt::Expr(ExprStmt {
                            span: *span,
                            expr: e.clone(),
                        })
                    })
                    .collect();
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
                if call.args.len() != 1 {
                    return None;
                }
                let Expr::Ident(exports_arg) = &*call.args[0].expr else {
                    return None;
                };
                if exports_arg.sym != self.exports_sym
                    || exports_arg.ctxt.outer() != self.unresolved_mark
                {
                    return None;
                }
                // Remove `require.r(exports)` entirely
                Some(vec![])
            }
            "d" => {
                // Convert `require.d(exports, "name", function() { return val; })` to `exports.name = val;`
                if call.args.len() != 3 {
                    return None;
                }
                let Expr::Ident(exports_arg) = &*call.args[0].expr else {
                    return None;
                };
                if exports_arg.sym != self.exports_sym
                    || exports_arg.ctxt.outer() != self.unresolved_mark
                {
                    return None;
                }
                let name_arg = &call.args[1];
                let getter_arg = &call.args[2];

                // Second arg must be a string literal (the export name)
                let Expr::Lit(Lit::Str(name_str)) = &*name_arg.expr else {
                    return None;
                };
                // name_str.value is Wtf8Atom; convert to Atom via &str
                let export_name: Atom =
                    name_str.value.as_str().map(Atom::from).unwrap_or_else(|| {
                        Atom::from(name_str.value.to_string_lossy().into_owned().as_str())
                    });

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
        if callee_ident.sym != self.require_sym || callee_ident.ctxt.outer() != self.unresolved_mark
        {
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
    getter_ids: std::collections::HashSet<(Atom, SyntaxContext)>,
}

impl VisitMut for RequireNRewriter {
    fn visit_mut_var_declarator(&mut self, decl: &mut VarDeclarator) {
        let Some(init) = &mut decl.init else {
            return;
        };

        if let Some(rewritten) = self.rewrite_require_n_expr(init.as_ref()) {
            if let Pat::Ident(binding) = &decl.name {
                self.getter_ids
                    .insert((binding.id.sym.clone(), binding.id.ctxt));
            }
            *init = Box::new(rewritten);
            return;
        }

        init.visit_mut_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Some(rewritten) = self.rewrite_require_n_expr(expr) {
            *expr = rewritten;
        }
    }
}

impl RequireNRewriter {
    fn rewrite_require_n_expr(&self, expr: &Expr) -> Option<Expr> {
        // Match: require.n(single_arg)
        let Expr::Call(call) = expr else { return None };
        if call.args.len() != 1 || call.args[0].spread.is_some() {
            return None;
        }
        let Callee::Expr(callee_expr) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
            return None;
        };
        let Expr::Ident(obj_ident) = &**obj else {
            return None;
        };
        if obj_ident.sym != self.require_sym || obj_ident.ctxt.outer() != self.unresolved_mark {
            return None;
        }
        let MemberProp::Ident(prop_ident) = prop else {
            return None;
        };
        if prop_ident.sym.as_ref() != "n" {
            return None;
        }

        let arg = call.args[0].expr.clone();
        let esmodule_check = Expr::Bin(BinExpr {
            span: Default::default(),
            op: BinaryOp::LogicalAnd,
            left: arg.clone(),
            right: Box::new(Expr::Member(MemberExpr {
                span: Default::default(),
                obj: arg.clone(),
                prop: MemberProp::Ident(IdentName::new("__esModule".into(), Default::default())),
            })),
        });
        let default_value = Expr::Member(MemberExpr {
            span: Default::default(),
            obj: arg.clone(),
            prop: MemberProp::Ident(IdentName::new("default".into(), Default::default())),
        });

        Some(Expr::Arrow(swc_core::ecma::ast::ArrowExpr {
            span: Default::default(),
            ctxt: Default::default(),
            params: vec![],
            body: Box::new(swc_core::ecma::ast::BlockStmtOrExpr::Expr(Box::new(
                Expr::Cond(CondExpr {
                    span: Default::default(),
                    test: Box::new(esmodule_check),
                    cons: Box::new(default_value),
                    alt: arg,
                }),
            ))),
            is_async: false,
            is_generator: false,
            type_params: None,
            return_type: None,
        }))
    }
}

/// Rewrites accesses like `getter.a` to `getter()`, where `getter` came from `require.n(...)`.
struct RequireNAccessRewriter {
    getter_ids: std::collections::HashSet<(Atom, SyntaxContext)>,
}

impl VisitMut for RequireNAccessRewriter {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Member(member) = expr else {
            return;
        };
        let Expr::Ident(obj_ident) = &*member.obj else {
            return;
        };
        if !self
            .getter_ids
            .contains(&(obj_ident.sym.clone(), obj_ident.ctxt))
        {
            return;
        }

        let is_accessor = match &member.prop {
            MemberProp::Ident(prop) => prop.sym.as_ref() == "a",
            MemberProp::Computed(prop) => matches!(
                &*prop.expr,
                Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some("a")
            ),
            _ => false,
        };
        if !is_accessor {
            return;
        }

        *expr = Expr::Call(CallExpr {
            span: Default::default(),
            ctxt: Default::default(),
            callee: Callee::Expr(Box::new(Expr::Ident(obj_ident.clone()))),
            args: vec![],
            type_args: None,
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
            Expr::Unary(u) if u.op == UnaryOp::Bang => extract_call_from_expr(&u.arg)?,
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
            Expr::Unary(u) if u.op == UnaryOp::Bang => extract_call_from_expr(&u.arg)?,
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
fn extract_webpack4_modules(
    call: &CallExpr,
    cm: Lrc<SourceMap>,
    apply_rules: bool,
) -> Option<UnpackResult> {
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
    let module_fns: Vec<Option<&FnExpr>> = array_lit
        .elems
        .iter()
        .map(|elem| {
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
        })
        .collect();

    // Validate: at least one function with at least 1 param
    let has_module_fn = module_fns.iter().any(|f| {
        f.map(|fn_expr| !fn_expr.function.params.is_empty())
            .unwrap_or(false)
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
        let param_syms: Vec<Atom> = params
            .iter()
            .filter_map(|p| {
                if let Pat::Ident(bi) = &p.pat {
                    Some(bi.sym.clone())
                } else {
                    None
                }
            })
            .collect();

        // Build renaming map: param[0] -> "module", param[1] -> "exports", param[2] -> "require"
        let standard_names = ["module", "exports", "require"];
        let renames: Vec<(Atom, Atom)> = param_syms
            .iter()
            .enumerate()
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
            let orig = param_syms
                .get(1)
                .cloned()
                .unwrap_or_else(|| Atom::from("exports"));
            // After renaming, it will be "exports" if there's a rename, else whatever it was
            if renames.iter().any(|(old, _)| old == &orig) {
                Atom::from("exports")
            } else {
                orig
            }
        };
        let require_sym = {
            let orig = param_syms
                .get(2)
                .cloned()
                .unwrap_or_else(|| Atom::from("require"));
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
        // locals) get a different mark.
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        synthetic_module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        // Step 1: rename factory params to standard names using swc_ecma_utils::replace_ident.
        // replace_ident matches on Id = (Atom, SyntaxContext), so it is inherently scope-aware:
        // only the specific unresolved free-variable references are renamed, not any inner
        // bindings that happen to share the same symbol.
        let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
        for (old_sym, new_sym) in &renames {
            let from_id = (old_sym.clone(), unresolved_ctxt);
            // Preserve unresolved_ctxt so downstream visitors (RequireIdRewriter, etc.)
            // can still match on `id.ctxt.outer() == unresolved_mark`.
            let to_ident = Ident::new(new_sym.clone(), Default::default(), unresolved_ctxt);
            replace_ident(&mut synthetic_module, from_id, &to_ident);
        }

        // Step 1b: rewrite require(N) → require("./module-N.js") so un-esm can convert them
        // After step 1, the require parameter is now named "require"
        let post_rename_require_sym = if param_syms.get(2).map(|s| s.as_ref()) != Some("require") {
            Atom::from("require")
        } else {
            param_syms
                .get(2)
                .cloned()
                .unwrap_or_else(|| Atom::from("require"))
        };
        {
            let mut id_rewriter = RequireIdRewriter {
                require_sym: post_rename_require_sym.clone(),
                unresolved_mark,
                id_to_filename: &id_to_filename,
            };
            synthetic_module.visit_mut_with(&mut id_rewriter);
        }

        // Step 1c: rewrite require.n(expr) to an explicit getter and normalize `.a` accesses.
        {
            let mut n_rewriter = RequireNRewriter {
                require_sym: post_rename_require_sym.clone(),
                unresolved_mark,
                getter_ids: std::collections::HashSet::new(),
            };
            synthetic_module.visit_mut_with(&mut n_rewriter);
            if !n_rewriter.getter_ids.is_empty() {
                let mut access_rewriter = RequireNAccessRewriter {
                    getter_ids: n_rewriter.getter_ids,
                };
                synthetic_module.visit_mut_with(&mut access_rewriter);
            }
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

        // Step 2b: strip webpack's `.call(this, require(G), require(A)(module))`
        // global-polyfill envelope when its fingerprint matches. Runs before
        // default rules so the rules pipeline sees a cleaner top-level shape.
        unwrap_global_polyfill(&mut synthetic_module, unresolved_mark);

        // Step 3: optionally apply default rules
        if apply_rules {
            apply_default_rules(&mut synthetic_module, unresolved_mark);
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
            id: idx.to_string(),
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

    let declared_idents = collect_declared_idents(&body.stmts);
    let called_idents = collect_called_idents(&body.stmts);
    let allowed_entry_objects: HashSet<Atom> = declared_idents
        .intersection(&called_idents)
        .cloned()
        .collect();

    let mut entries = Vec::new();
    for stmt in &body.stmts {
        collect_entry_ids_from_stmt(stmt, &allowed_entry_objects, &mut entries);
    }
    entries
}

fn collect_entry_ids_from_stmt(
    stmt: &Stmt,
    allowed_entry_objects: &HashSet<Atom>,
    entries: &mut Vec<usize>,
) {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return;
    };
    collect_entry_ids_from_expr(expr, allowed_entry_objects, entries);
}

fn collect_entry_ids_from_expr(
    expr: &Expr,
    allowed_entry_objects: &HashSet<Atom>,
    entries: &mut Vec<usize>,
) {
    match expr {
        // `n(n.s = 51)` — call where arg is assignment to <fn>.s
        Expr::Call(call) => {
            for arg in &call.args {
                collect_entry_ids_from_expr(&arg.expr, allowed_entry_objects, entries);
            }
        }
        // `n.s = 51` at statement level
        Expr::Assign(assign) => {
            if let Some(id) = extract_entry_id_from_assign(assign, allowed_entry_objects) {
                entries.push(id);
            }
        }
        // Sequences like `n.m=e, n.c=t, ..., n(n.s=51)`
        Expr::Seq(seq) => {
            for e in &seq.exprs {
                collect_entry_ids_from_expr(e, allowed_entry_objects, entries);
            }
        }
        _ => {}
    }
}

fn extract_entry_id_from_assign(
    assign: &AssignExpr,
    allowed_entry_objects: &HashSet<Atom>,
) -> Option<usize> {
    if assign.op != AssignOp::Assign {
        return None;
    }
    // Left must be MemberExpr with prop "s"
    let AssignTarget::Simple(SimpleAssignTarget::Member(m)) = &assign.left else {
        return None;
    };
    let Expr::Ident(obj_ident) = &*m.obj else {
        return None;
    };
    if !allowed_entry_objects.contains(&obj_ident.sym) {
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

fn collect_declared_idents(stmts: &[Stmt]) -> HashSet<Atom> {
    let mut names = HashSet::new();
    for stmt in stmts {
        match stmt {
            Stmt::Decl(swc_core::ecma::ast::Decl::Fn(fn_decl)) => {
                names.insert(fn_decl.ident.sym.clone());
            }
            Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl)) => {
                for decl in &var_decl.decls {
                    if let Pat::Ident(binding) = &decl.name {
                        names.insert(binding.id.sym.clone());
                    }
                }
            }
            _ => {}
        }
    }
    names
}

fn collect_called_idents(stmts: &[Stmt]) -> HashSet<Atom> {
    let mut names = HashSet::new();
    for stmt in stmts {
        collect_called_idents_from_stmt(stmt, &mut names);
    }
    names
}

fn collect_called_idents_from_stmt(stmt: &Stmt, names: &mut HashSet<Atom>) {
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        collect_called_idents_from_expr(expr, names);
    }
}

fn collect_called_idents_from_expr(expr: &Expr, names: &mut HashSet<Atom>) {
    match expr {
        Expr::Call(call) => {
            if let Callee::Expr(callee_expr) = &call.callee {
                if let Expr::Ident(id) = &**callee_expr {
                    names.insert(id.sym.clone());
                }
            }
            for arg in &call.args {
                collect_called_idents_from_expr(&arg.expr, names);
            }
        }
        Expr::Assign(assign) => collect_called_idents_from_expr(&assign.right, names),
        Expr::Seq(seq) => {
            for expr in &seq.exprs {
                collect_called_idents_from_expr(expr, names);
            }
        }
        _ => {}
    }
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

// ============================================================
// webpack4 "global polyfill" wrapper unwrapping
// ============================================================
//
// webpack4 emits global-detecting modules with a distinctive envelope:
//
//     (function(e, r) {
//         // ... uses `e` and `r` only as fallback globals ...
//         o = typeof self !== "undefined"
//             ? self
//             : typeof window !== "undefined"
//                 ? window
//                 : e !== undefined ? e : r;
//         // ... rest of body ...
//     }).call(this, require("./module-42.js"), require("./module-41.js")(module));
//
// This is recognizable without a bundle-wide helper-module registry because:
// - `.call(this, ...)` at top-level with a fn/arrow IIFE base is narrow
// - `require(X)(<Ident "module">)` as an arg is essentially never user code
//   — the raw `module` binding only exists inside a CommonJS wrapper, and
//   the AMD-define polyfill is the one thing that consumes it
// - the `typeof self → typeof window → param → param` ternary is webpack's
//   own global-detection template, referenced only via the IIFE's params
//
// When all three tells line up, the wrapper is dead weight: the ternary
// resolves to `globalThis` in any post-ES2020 runtime, and the `e`/`r`
// arguments are only consumed as fallback arms of that same ternary. We
// hoist the body to module scope and replace the ternary with `globalThis`.

/// Strip webpack4's global-polyfill IIFE wrapper on matching top-level
/// statements. Called once per extracted module, after the webpack runtime
/// normalizer and before `apply_default_rules`.
fn unwrap_global_polyfill(module: &mut Module, unresolved_mark: Mark) {
    let mut new_body: Vec<ModuleItem> = Vec::with_capacity(module.body.len());
    for item in module.body.drain(..) {
        match try_unwrap_polyfill_item(&item, unresolved_mark) {
            Some(replacement) => new_body.extend(replacement),
            None => new_body.push(item),
        }
    }
    module.body = new_body;
}

fn try_unwrap_polyfill_item(item: &ModuleItem, unresolved_mark: Mark) -> Option<Vec<ModuleItem>> {
    let ModuleItem::Stmt(stmt) = item else {
        return None;
    };
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };

    // Callee must be `<fn|arrow>.call`.
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee_expr.as_ref() else {
        return None;
    };
    let MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym.as_ref() != "call" {
        return None;
    }

    let (param_ids, body_stmts) = extract_inner_callee(&member.obj)?;
    // Can't verify param usage on a paramless wrapper — not our pattern anyway.
    if param_ids.is_empty() {
        return None;
    }

    // Arg list: first must be literal `this`, none may be spreads.
    if call.args.is_empty() || call.args[0].spread.is_some() {
        return None;
    }
    if !matches!(&*call.args[0].expr, Expr::This(_)) {
        return None;
    }
    if call.args.iter().skip(1).any(|a| a.spread.is_some()) {
        return None;
    }

    // The defining fingerprint: at least one arg is `require(<str>)(module)`.
    // User code essentially never invokes a `require()` result with the raw
    // `module` binding — this shape is the webpack AMD-define polyfill.
    if !call
        .args
        .iter()
        .skip(1)
        .any(|a| is_require_invoked_with_module(&a.expr))
    {
        return None;
    }

    // Rewrite a clone: replace the global-detect ternary with `globalThis`.
    // If no ternary matches our signature, bail — we don't want to strip the
    // wrapper without also neutralizing the param references inside it.
    let mut candidate = body_stmts.clone();
    let mut replacer = TernaryReplacer {
        param_ids: &param_ids,
        unresolved_mark,
        replacements: 0,
    };
    for stmt in &mut candidate {
        stmt.visit_mut_with(&mut replacer);
    }
    if replacer.replacements == 0 {
        return None;
    }

    // After replacing the ternary(s), no param reference may remain — if
    // anything survives, the wrapper was carrying real data and we can't
    // safely drop its arguments.
    let mut counter = ParamRefCounter {
        param_ids: &param_ids,
        count: 0,
    };
    for stmt in &candidate {
        stmt.visit_with(&mut counter);
    }
    if counter.count > 0 {
        return None;
    }

    Some(candidate.into_iter().map(ModuleItem::Stmt).collect())
}

/// Extract `(param_ids, body_stmts)` from an expression that is expected to
/// be a fn/arrow IIFE callee. Skips a surrounding paren wrapper. Arrow
/// expression-bodies are rejected — the wrapper pattern always has a block.
fn extract_inner_callee(expr: &Expr) -> Option<(Vec<Id>, Vec<Stmt>)> {
    let mut unwrapped = expr;
    while let Expr::Paren(p) = unwrapped {
        unwrapped = p.expr.as_ref();
    }
    match unwrapped {
        Expr::Fn(FnExpr { function, .. }) => {
            let params = collect_param_ids(function.params.iter().map(|p| &p.pat))?;
            let body = function.body.as_ref()?.stmts.clone();
            Some((params, body))
        }
        Expr::Arrow(ArrowExpr { params, body, .. }) => {
            let param_ids = collect_param_ids(params.iter())?;
            let BlockStmtOrExpr::BlockStmt(BlockStmt { stmts, .. }) = body.as_ref() else {
                return None;
            };
            Some((param_ids, stmts.clone()))
        }
        _ => None,
    }
}

fn collect_param_ids<'a, I: Iterator<Item = &'a Pat>>(pats: I) -> Option<Vec<Id>> {
    let mut out = Vec::new();
    for pat in pats {
        let Pat::Ident(bi) = pat else {
            return None;
        };
        out.push((bi.id.sym.clone(), bi.id.ctxt));
    }
    Some(out)
}

/// `require(<string literal>)(module)` — the AMD-define polyfill call site.
/// The tail arg must be the raw `module` identifier; `module.exports` or any
/// other shape doesn't qualify.
fn is_require_invoked_with_module(expr: &Expr) -> bool {
    let Expr::Call(outer) = expr else {
        return false;
    };
    if outer.args.len() != 1 || outer.args[0].spread.is_some() {
        return false;
    }
    let Expr::Ident(arg_ident) = &*outer.args[0].expr else {
        return false;
    };
    if arg_ident.sym.as_ref() != "module" {
        return false;
    }

    // Inner: require("./…")
    let Callee::Expr(outer_callee) = &outer.callee else {
        return false;
    };
    let Expr::Call(inner) = outer_callee.as_ref() else {
        return false;
    };
    let Callee::Expr(inner_callee) = &inner.callee else {
        return false;
    };
    let Expr::Ident(id) = inner_callee.as_ref() else {
        return false;
    };
    if id.sym.as_ref() != "require" {
        return false;
    }
    inner.args.len() == 1
        && inner.args[0].spread.is_none()
        && matches!(&*inner.args[0].expr, Expr::Lit(Lit::Str(_)))
}

// ---- ternary matching ----

/// Matches the inner `<param> !== undefined ? <param> : <param>` arm and
/// returns whether both leaf idents are among `param_ids`.
fn is_param_fallback_cond(cond: &CondExpr, param_ids: &[Id]) -> bool {
    // Test: `P !== undefined` (either order, with `void 0` accepted as undefined).
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = cond.test.as_ref()
    else {
        return false;
    };
    if !matches!(op, BinaryOp::NotEq | BinaryOp::NotEqEq) {
        return false;
    }
    let test_ident = match (is_undefined_expr(left), is_undefined_expr(right)) {
        (true, false) => right.as_ref(),
        (false, true) => left.as_ref(),
        _ => return false,
    };
    let Expr::Ident(test_id) = test_ident else {
        return false;
    };
    if !id_in(param_ids, test_id) {
        return false;
    }

    // `cons` should reference the same param as the test.
    let Expr::Ident(cons_id) = cond.cons.as_ref() else {
        return false;
    };
    if (cons_id.sym.clone(), cons_id.ctxt) != (test_id.sym.clone(), test_id.ctxt) {
        return false;
    }

    // `alt` should be another param (the second fallback).
    let Expr::Ident(alt_id) = cond.alt.as_ref() else {
        return false;
    };
    id_in(param_ids, alt_id)
}

fn is_undefined_expr(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Void,
            arg,
            ..
        }) if matches!(arg.as_ref(), Expr::Lit(Lit::Num(_)))
    ) || matches!(expr, Expr::Ident(id) if id.sym.as_ref() == "undefined")
}

/// `typeof <Ident> != "undefined"` — accepts either operand order and both
/// `!=` / `!==` forms (`===` is webpack's actual emission for the outer
/// arms, but raw output can normalize to `==` depending on what ran before).
fn matches_typeof_defined(expr: &Expr, expected: &str) -> bool {
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return false;
    };
    if !matches!(op, BinaryOp::NotEq | BinaryOp::NotEqEq) {
        return false;
    }
    let (typeof_side, lit_side) = match (is_string_lit(left, "undefined"), is_string_lit(right, "undefined")) {
        (true, false) => (right.as_ref(), left.as_ref()),
        (false, true) => (left.as_ref(), right.as_ref()),
        _ => return false,
    };
    let _ = lit_side;
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::TypeOf,
        arg,
        ..
    }) = typeof_side
    else {
        return false;
    };
    matches!(arg.as_ref(), Expr::Ident(id) if id.sym.as_ref() == expected)
}

fn is_string_lit(expr: &Expr, value: &str) -> bool {
    if let Expr::Lit(Lit::Str(s)) = expr {
        s.value.as_str().is_some_and(|v| v == value)
    } else {
        false
    }
}

fn id_in(param_ids: &[Id], ident: &Ident) -> bool {
    param_ids
        .iter()
        .any(|(sym, ctxt)| *sym == ident.sym && *ctxt == ident.ctxt)
}

/// Matches the full `typeof self → typeof window → param-fallback` ternary.
fn is_global_detect_ternary(cond: &CondExpr, param_ids: &[Id]) -> bool {
    // Outer: `typeof self != "undefined" ? self : <inner>`
    if !matches_typeof_defined(&cond.test, "self") {
        return false;
    }
    if !matches!(cond.cons.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "self") {
        return false;
    }
    let Expr::Cond(inner) = cond.alt.as_ref() else {
        return false;
    };
    // Middle: `typeof window != "undefined" ? window : <param-fallback>`
    if !matches_typeof_defined(&inner.test, "window") {
        return false;
    }
    if !matches!(inner.cons.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "window") {
        return false;
    }
    let Expr::Cond(fallback) = inner.alt.as_ref() else {
        return false;
    };
    is_param_fallback_cond(fallback, param_ids)
}

struct TernaryReplacer<'a> {
    param_ids: &'a [Id],
    unresolved_mark: Mark,
    replacements: usize,
}

impl<'a> VisitMut for TernaryReplacer<'a> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        if let Expr::Cond(cond) = expr {
            if is_global_detect_ternary(cond, self.param_ids) {
                let ctxt = SyntaxContext::empty().apply_mark(self.unresolved_mark);
                *expr = Expr::Ident(Ident::new(Atom::from("globalThis"), Default::default(), ctxt));
                self.replacements += 1;
            }
        }
    }
}

struct ParamRefCounter<'a> {
    param_ids: &'a [Id],
    count: usize,
}

impl<'a> Visit for ParamRefCounter<'a> {
    fn visit_ident(&mut self, ident: &Ident) {
        if id_in(self.param_ids, ident) {
            self.count += 1;
        }
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

#[cfg(test)]
mod polyfill_tests {
    use super::*;

    fn run_unwrap(source: &str) -> String {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_es_module(source, cm.clone()).expect("parse");
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
            unwrap_global_polyfill(&mut module, unresolved_mark);
            module.visit_mut_with(&mut fixer(None));
            emit_module(&module, cm).expect("emit")
        })
    }

    // ---- positive cases ----

    #[test]
    fn unwraps_module_21_shape_function() {
        let input = r#"(function(e, r) {
    var o, i = require("./module-31.js");
    o = typeof self != "undefined" ? self : typeof window != "undefined" ? window : void 0 !== e ? e : r;
    exports.a = i.a(o);
}).call(this, require("./module-42.js"), require("./module-41.js")(module));
"#;
        let out = run_unwrap(input);
        assert!(!out.contains(".call(this"), "wrapper should be stripped: {out}");
        assert!(out.contains("globalThis"), "ternary should collapse to globalThis: {out}");
        assert!(out.contains(r#"require("./module-31.js")"#), "real import preserved: {out}");
        assert!(!out.contains(r#"require("./module-41.js")"#), "amd helper arg dropped: {out}");
    }

    #[test]
    fn unwraps_arrow_callee() {
        let input = r#"((e, r) => {
    let o = typeof self != "undefined" ? self : typeof window != "undefined" ? window : void 0 !== e ? e : r;
    exports.g = o;
}).call(this, globalA, require("./amd.js")(module));
"#;
        let out = run_unwrap(input);
        assert!(!out.contains(".call(this"), "{out}");
        assert!(out.contains("globalThis"), "{out}");
    }

    #[test]
    fn unwraps_paren_wrapped_callee() {
        let input = r#"((function(e, r) {
    var o = typeof self != "undefined" ? self : typeof window != "undefined" ? window : void 0 !== e ? e : r;
    exports.g = o;
})).call(this, g, require("./amd.js")(module));
"#;
        let out = run_unwrap(input);
        assert!(!out.contains(".call(this"), "paren-wrapped callee should still match: {out}");
    }

    #[test]
    fn unwraps_strict_equality_typeof_check() {
        // Webpack can emit `!==` on the typeof arms; our matcher accepts both.
        let input = r#"(function(e, r) {
    var o = typeof self !== "undefined" ? self : typeof window !== "undefined" ? window : void 0 !== e ? e : r;
    exports.g = o;
}).call(this, g, require("./amd.js")(module));
"#;
        let out = run_unwrap(input);
        assert!(!out.contains(".call(this"), "{out}");
    }

    // ---- negative cases ----

    #[test]
    fn skips_when_module_helper_tail_missing() {
        // Without a `require(X)(module)` arg, we don't trust the shape.
        let input = r#"(function(e, r) {
    var o = typeof self != "undefined" ? self : typeof window != "undefined" ? window : void 0 !== e ? e : r;
    exports.g = o;
}).call(this, something, somethingElse);
"#;
        let out = run_unwrap(input);
        assert!(out.contains(".call(this"), "should preserve wrapper: {out}");
    }

    #[test]
    fn skips_when_module_arg_is_not_raw_module_binding() {
        // `require(X)(module.exports)` doesn't count — the tell is the raw
        // `module` binding.
        let input = r#"(function(e, r) {
    var o = typeof self != "undefined" ? self : typeof window != "undefined" ? window : void 0 !== e ? e : r;
    exports.g = o;
}).call(this, g, require("./amd.js")(module.exports));
"#;
        let out = run_unwrap(input);
        assert!(out.contains(".call(this"), "{out}");
    }

    #[test]
    fn skips_when_global_ternary_missing() {
        // `(module)` tail is present but the body doesn't have the global-
        // detect ternary — we can't safely strip the args.
        let input = r#"(function(e, r) {
    exports.combined = e + r;
}).call(this, g, require("./amd.js")(module));
"#;
        let out = run_unwrap(input);
        assert!(out.contains(".call(this"), "{out}");
    }

    #[test]
    fn skips_dot_apply_variant() {
        // `.apply` takes an array, not positional args — different semantics.
        let input = r#"(function(e, r) {
    var o = typeof self != "undefined" ? self : typeof window != "undefined" ? window : void 0 !== e ? e : r;
    exports.g = o;
}).apply(this, [g, require("./amd.js")(module)]);
"#;
        let out = run_unwrap(input);
        assert!(out.contains(".apply"), "{out}");
    }

    #[test]
    fn skips_when_this_arg_is_not_this() {
        let input = r#"(function(e, r) {
    var o = typeof self != "undefined" ? self : typeof window != "undefined" ? window : void 0 !== e ? e : r;
    exports.g = o;
}).call(null, g, require("./amd.js")(module));
"#;
        let out = run_unwrap(input);
        assert!(out.contains(".call(null"), "non-this thisArg should preserve wrapper: {out}");
    }

    #[test]
    fn skips_when_params_used_outside_ternary() {
        // `r` is also assigned to a property — the wrapper carries real data,
        // not just a fallback global. Dropping the arg would change semantics.
        let input = r#"(function(e, r) {
    var o = typeof self != "undefined" ? self : typeof window != "undefined" ? window : void 0 !== e ? e : r;
    exports.g = o;
    exports.helper = r;
}).call(this, g, require("./amd.js")(module));
"#;
        let out = run_unwrap(input);
        assert!(out.contains(".call(this"), "param used outside ternary — preserve: {out}");
    }

    #[test]
    fn skips_when_wrapper_is_nested_not_top_level() {
        // Our pass only touches top-level module items. A nested occurrence
        // (inside another function) is untouched.
        let input = r#"function outer() {
    (function(e, r) {
        var o = typeof self != "undefined" ? self : typeof window != "undefined" ? window : void 0 !== e ? e : r;
        exports.g = o;
    }).call(this, g, require("./amd.js")(module));
}
"#;
        let out = run_unwrap(input);
        assert!(out.contains(".call(this"), "nested wrapper must not be stripped: {out}");
    }

    #[test]
    fn skips_when_param_fallback_cons_does_not_match_test() {
        // `void 0 !== e ? r : e` — cons/test mismatch breaks the fingerprint.
        let input = r#"(function(e, r) {
    var o = typeof self != "undefined" ? self : typeof window != "undefined" ? window : void 0 !== e ? r : e;
    exports.g = o;
}).call(this, g, require("./amd.js")(module));
"#;
        let out = run_unwrap(input);
        assert!(out.contains(".call(this"), "{out}");
    }
}
