use std::collections::HashMap;

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, BindingIdent, BlockStmtOrExpr, ComputedPropName, Decl, Expr, ExprStmt, Ident,
    KeyValuePatProp, Lit, MemberExpr, MemberProp, Module, ModuleItem, Number, ObjectPat,
    ObjectPatProp, Pat, PropName, Stmt, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub struct SmartInline;

impl VisitMut for SmartInline {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // Step 0: Inline zero-param arrow ident wrappers (const X = () => Y) globally.
        // These are often produced by `require.n` rewriting and used inside nested functions,
        // so they need cross-boundary inlining before per-stmt processing.
        inline_module_arrow_wrappers(module);

        // Process module-level statements
        let stmts: Vec<Stmt> = module
            .body
            .iter()
            .filter_map(|item| {
                if let ModuleItem::Stmt(s) = item {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect();

        let new_stmts = process_stmts(stmts);

        // Rebuild module body
        let mut new_body = Vec::new();
        let mut stmt_idx = 0;
        for item in module.body.drain(..) {
            match item {
                ModuleItem::Stmt(_) => {
                    if stmt_idx < new_stmts.len() {
                        new_body.push(ModuleItem::Stmt(new_stmts[stmt_idx].clone()));
                        stmt_idx += 1;
                    }
                }
                other => new_body.push(other),
            }
        }
        // Add any remaining (new_stmts may be longer after splitting)
        while stmt_idx < new_stmts.len() {
            new_body.push(ModuleItem::Stmt(new_stmts[stmt_idx].clone()));
            stmt_idx += 1;
        }
        module.body = new_body;

        module.visit_mut_children_with(self);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        let taken = std::mem::take(stmts);
        *stmts = process_stmts(taken);
        stmts.visit_mut_children_with(self);
    }
}

// ============================================================
// Main processing pipeline per statement list
// ============================================================

fn process_stmts(stmts: Vec<Stmt>) -> Vec<Stmt> {
    // Pass 1: inline single-use const declarations (temp vars)
    let stmts = inline_temp_vars(stmts);
    // Pass 2: group consecutive property / array accesses into destructuring
    let stmts = group_destructuring(stmts);
    stmts
}

// ============================================================
// Module-level arrow wrapper inlining
// Handles: const X = () => Y  (zero-param arrow with ident body)
// These are typically require.n-generated getters used inside nested functions.
// Inlines globally (including across nested function/arrow boundaries).
// After inlining, the second UnIife pass converts (() => Y)(...) → Y(...).
// ============================================================

fn try_extract_zero_param_arrow_ident(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Arrow(arrow) = expr else { return None };
    if !arrow.params.is_empty() {
        return None;
    }
    if let BlockStmtOrExpr::Expr(body_expr) = arrow.body.as_ref() {
        if matches!(body_expr.as_ref(), Expr::Ident(_)) {
            return Some(body_expr.clone());
        }
    }
    None
}

/// Scope-aware key for arrow wrapper candidates: (symbol, SyntaxContext from resolver).
type BindingKey = (Atom, SyntaxContext);

fn inline_module_arrow_wrappers(module: &mut Module) {
    // Collect candidates: const X = () => identY at module level (Stmt items only).
    // Use (sym, ctxt) keys so inner-scope variables with the same name are NOT replaced.
    let mut candidates: HashMap<BindingKey, Box<Expr>> = HashMap::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else { continue };
        if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
            continue;
        }
        let decl = &var.decls[0];
        let Pat::Ident(bi) = &decl.name else { continue };
        let Some(init) = &decl.init else { continue };
        if try_extract_zero_param_arrow_ident(init).is_some() {
            candidates.insert((bi.id.sym.clone(), bi.id.ctxt), init.clone());
        }
    }

    if candidates.is_empty() {
        return;
    }

    // Count usages globally (including inside nested functions), excluding the definition stmts.
    // Keyed by (sym, ctxt) so only the exact binding is counted.
    let mut usage_count: HashMap<BindingKey, usize> =
        candidates.keys().map(|k| (k.clone(), 0)).collect();

    for item in &module.body {
        // Skip the definition stmt itself
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                if let Pat::Ident(bi) = &var.decls[0].name {
                    if candidates.contains_key(&(bi.id.sym.clone(), bi.id.ctxt)) {
                        continue;
                    }
                }
            }
        }
        let mut counter = GlobalIdentCounter { counts: &mut usage_count };
        item.visit_with(&mut counter);
    }

    // Keep only those with at least one usage elsewhere
    let to_inline: HashMap<BindingKey, Box<Expr>> = candidates
        .into_iter()
        .filter(|(key, _)| usage_count.get(key).copied().unwrap_or(0) >= 1)
        .collect();

    if to_inline.is_empty() {
        return;
    }

    // Remove the definition stmts from the module body
    module.body.retain(|item| {
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                if let Pat::Ident(bi) = &var.decls[0].name {
                    if to_inline.contains_key(&(bi.id.sym.clone(), bi.id.ctxt)) {
                        return false;
                    }
                }
            }
        }
        true
    });

    // Replace all usages globally (including inside nested functions)
    let mut inliner = GlobalIdentInliner { map: &to_inline };
    module.visit_mut_with(&mut inliner);
}

/// Counts ident usages everywhere, including inside nested functions/arrows.
/// Uses (sym, ctxt) keys for scope-aware matching.
struct GlobalIdentCounter<'a> {
    counts: &'a mut HashMap<BindingKey, usize>,
}

impl Visit for GlobalIdentCounter<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        if let Some(c) = self.counts.get_mut(&(id.sym.clone(), id.ctxt)) {
            *c += 1;
        }
    }
    // Skip non-computed member props and prop names (not value positions)
    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }
    fn visit_prop_name(&mut self, _: &PropName) {}
}

/// Replaces ident usages everywhere, including inside nested functions/arrows.
/// Uses (sym, ctxt) keys for scope-aware matching.
struct GlobalIdentInliner<'a> {
    map: &'a HashMap<BindingKey, Box<Expr>>,
}

impl GlobalIdentInliner<'_> {
    /// If `replacement` is `() => ident`, return the inner ident expr.
    fn inner_ident(replacement: &Expr) -> Option<&Expr> {
        let Expr::Arrow(arrow) = replacement else { return None };
        if !arrow.params.is_empty() { return None; }
        if let BlockStmtOrExpr::Expr(body) = arrow.body.as_ref() {
            if matches!(body.as_ref(), Expr::Ident(_)) {
                return Some(body.as_ref());
            }
        }
        None
    }
}

impl VisitMut for GlobalIdentInliner<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        // Handle `X.a` where X is a zero-param arrow wrapper candidate.
        // In webpack4, `require.n(m).a` is a property getter that returns the module,
        // equivalent to calling the getter directly. Replace `X.a` with the inner ident.
        if let Expr::Member(member) = expr {
            let is_dot_a = match &member.prop {
                MemberProp::Ident(p) => p.sym.as_ref() == "a",
                MemberProp::Computed(c) => matches!(
                    c.expr.as_ref(),
                    Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some("a")
                ),
                _ => false,
            };
            if is_dot_a {
                if let Expr::Ident(id) = member.obj.as_ref() {
                    let key = (id.sym.clone(), id.ctxt);
                    if let Some(replacement) = self.map.get(&key) {
                        if let Some(inner) = Self::inner_ident(replacement) {
                            *expr = inner.clone();
                            return;
                        }
                    }
                }
            }
        }

        // Regular ident replacement: X → () => Y (call sites become (() => Y)() → Y via UnIife)
        if let Expr::Ident(id) = expr {
            let key = (id.sym.clone(), id.ctxt);
            if let Some(replacement) = self.map.get(&key) {
                *expr = *replacement.clone();
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }
    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_mut_with(self);
        }
    }
    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}
    // NOTE: intentionally does NOT stop at function/arrow/class boundaries
}

// ============================================================
// Pass 1: Temp variable inlining
// ============================================================

fn inline_temp_vars(stmts: Vec<Stmt>) -> Vec<Stmt> {
    // Collect candidates: `const t = e` where e is a simple expr (Ident or Lit)
    // Only inline if t is used exactly once in the rest of the block (not in nested functions)
    let mut candidates: HashMap<Atom, Box<Expr>> = HashMap::new();

    // First pass: find all `const t = e` with simple inits and count usages
    for stmt in &stmts {
        if let Stmt::Decl(Decl::Var(var)) = stmt {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                let decl = &var.decls[0];
                if let Pat::Ident(bi) = &decl.name {
                    if let Some(init) = &decl.init {
                        if is_simple_expr(init) {
                            candidates.insert(bi.id.sym.clone(), init.clone());
                        }
                    }
                }
            }
        }
    }

    if candidates.is_empty() {
        return stmts;
    }

    // Count top-level usages for each candidate
    let mut usage_count: HashMap<Atom, usize> = HashMap::new();
    for name in candidates.keys() {
        usage_count.insert(name.clone(), 0);
    }

    for stmt in &stmts {
        count_top_level_ident_uses_in_stmt(stmt, &mut usage_count);
    }

    // Candidates with exactly 1 usage (including their own declaration) →
    // the declaration counts as 1 "definition" not a "use", so we count uses in
    // non-declaration stmts only. Actually let's recount: only count uses NOT in the def stmt.

    // Re-count: skip the definition statement itself
    let mut usage_count2: HashMap<Atom, usize> = HashMap::new();
    for name in candidates.keys() {
        usage_count2.insert(name.clone(), 0);
    }

    for stmt in &stmts {
        // Skip definition stmts
        if let Stmt::Decl(Decl::Var(var)) = stmt {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                if let Pat::Ident(bi) = &var.decls[0].name {
                    if candidates.contains_key(&bi.id.sym) {
                        continue; // skip this stmt
                    }
                }
            }
        }
        count_top_level_ident_uses_in_stmt(stmt, &mut usage_count2);
    }

    // Build set of names to inline (exactly 1 top-level use)
    let to_inline: HashMap<Atom, Box<Expr>> = candidates
        .into_iter()
        .filter(|(name, _)| usage_count2.get(name).copied().unwrap_or(0) == 1)
        .collect();

    if to_inline.is_empty() {
        return stmts;
    }

    // Apply inlining: remove definition stmts, replace single usage with init expr
    let mut result = Vec::new();
    for stmt in stmts {
        // Skip definitions of inlined vars
        if let Stmt::Decl(Decl::Var(var)) = &stmt {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                if let Pat::Ident(bi) = &var.decls[0].name {
                    if to_inline.contains_key(&bi.id.sym) {
                        continue;
                    }
                }
            }
        }
        let mut stmt = stmt;
        // Replace usages of inlined vars in this statement
        let mut inliner = IdentInliner { map: &to_inline };
        stmt.visit_mut_with(&mut inliner);
        result.push(stmt);
    }

    result
}

fn is_simple_expr(expr: &Expr) -> bool {
    // Only inline identifier aliases (const t = someVar), not literals.
    // Literal constants (const g = 'url', const n = 42) are intentionally named
    // and should not be collapsed back into their usage site.
    matches!(expr, Expr::Ident(_))
}

/// Count top-level ident usages (not inside nested function bodies).
fn count_top_level_ident_uses_in_stmt(stmt: &Stmt, counts: &mut HashMap<Atom, usize>) {
    let mut counter = TopLevelIdentCounter { counts };
    stmt.visit_with(&mut counter);
}

struct TopLevelIdentCounter<'a> {
    counts: &'a mut HashMap<Atom, usize>,
}

impl Visit for TopLevelIdentCounter<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        if let Some(c) = self.counts.get_mut(&id.sym) {
            *c += 1;
        }
    }
    // Don't descend into nested function bodies (closures capture by reference)
    fn visit_function(&mut self, _: &swc_core::ecma::ast::Function) {}
    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}
    // Don't rename property keys
    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }
    fn visit_prop_name(&mut self, _: &PropName) {}
}

struct IdentInliner<'a> {
    map: &'a HashMap<Atom, Box<Expr>>,
}

impl VisitMut for IdentInliner<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        // Replace ident with its mapped expr before recursing
        if let Expr::Ident(id) = expr {
            if let Some(replacement) = self.map.get(&id.sym) {
                *expr = *replacement.clone();
                return; // No need to recurse into the replacement
            }
        }
        expr.visit_mut_children_with(self);
    }
    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_mut_with(self);
        }
    }
    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}
    // Don't inline inside nested functions (would change closure semantics)
    fn visit_mut_function(&mut self, _: &mut swc_core::ecma::ast::Function) {}
    fn visit_mut_arrow_expr(&mut self, _: &mut swc_core::ecma::ast::ArrowExpr) {}
}

// ============================================================
// Pass 2: Group property / array accesses into destructuring
// ============================================================

#[derive(Debug, Clone)]
enum AccessKind {
    /// obj.prop or obj["prop"] — maps to (binding_name, prop_key_string)
    Property { binding: Option<Atom>, prop_key: PropKey },
    /// obj[n] — maps to (binding_name, index)
    Index { binding: Option<Atom>, index: usize },
}

#[derive(Debug, Clone)]
enum PropKey {
    Ident(Atom),
    Str(Atom),
}

fn group_destructuring(stmts: Vec<Stmt>) -> Vec<Stmt> {
    // Scan for groups of consecutive `const t = obj.prop` / `const t = obj[n]`
    // where `obj` is a plain identifier.
    // Group by the obj name, emit destructuring when group is "flushed".

    let mut result: Vec<Stmt> = Vec::new();
    let mut current_obj: Option<(Atom, Vec<AccessKind>)> = None;
    let mut i = 0;
    let stmts_count = stmts.len();

    while i < stmts_count {
        let stmt = &stmts[i];

        let next_access = try_extract_prop_access(stmt)
            .map(|(obj, key, binding)| (obj, AccessKind::Property { binding, prop_key: key }))
            .or_else(|| {
                try_extract_index_access(stmt)
                    .map(|(obj, index, binding)| (obj, AccessKind::Index { binding, index }))
            });

        if let Some((obj_name, access)) = next_access {
            match &mut current_obj {
                Some((cur_obj, accesses)) if cur_obj == &obj_name => {
                    accesses.push(access);
                }
                _ => {
                    if let Some((obj, acc)) = current_obj.take() {
                        flush_group(&mut result, obj, acc);
                    }
                    current_obj = Some((obj_name, vec![access]));
                }
            }
            i += 1;
            continue;
        }

        // Non-matching statement: flush current group
        if let Some((obj, acc)) = current_obj.take() {
            flush_group(&mut result, obj, acc);
        }
        result.push(stmts[i].clone());
        i += 1;
    }

    if let Some((obj, acc)) = current_obj.take() {
        flush_group(&mut result, obj, acc);
    }

    result
}

/// Try to extract `const t = obj.prop`
/// Returns `(obj_ident, prop_key, binding_name)`
fn try_extract_prop_access(stmt: &Stmt) -> Option<(Atom, PropKey, Option<Atom>)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else { return None };
    if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(bi) = &decl.name else { return None };
    let init = decl.init.as_ref()?;
    let (obj_name, prop_key) = extract_obj_prop(init)?;
    Some((obj_name, prop_key, Some(bi.id.sym.clone())))
}

fn extract_obj_prop(expr: &Expr) -> Option<(Atom, PropKey)> {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return None;
    };
    // obj must be a plain identifier
    let Expr::Ident(obj_id) = obj.as_ref() else {
        return None;
    };
    let key = match prop {
        MemberProp::Ident(ident_name) => PropKey::Ident(ident_name.sym.clone()),
        MemberProp::Computed(computed) => {
            // Only handle string literal keys
            let Expr::Lit(Lit::Str(s)) = computed.expr.as_ref() else {
                return None;
            };
            let s_str = s.value.as_str()?.to_string();
            PropKey::Str(s_str.as_str().into())
        }
        _ => return None,
    };
    Some((obj_id.sym.clone(), key))
}

/// Try to extract `const t = obj[n]` where n is a numeric literal ≤10
fn try_extract_index_access(stmt: &Stmt) -> Option<(Atom, usize, Option<Atom>)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else { return None };
    if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(bi) = &decl.name else { return None };
    let init = decl.init.as_ref()?;

    let Expr::Member(MemberExpr { obj, prop, .. }) = init.as_ref() else {
        return None;
    };
    let Expr::Ident(obj_id) = obj.as_ref() else { return None };
    let MemberProp::Computed(computed) = prop else { return None };
    let Expr::Lit(Lit::Num(Number { value, .. })) = computed.expr.as_ref() else {
        return None;
    };
    let idx = *value as usize;
    if idx > 10 || *value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    Some((obj_id.sym.clone(), idx, Some(bi.id.sym.clone())))
}

/// Determine if accesses are all Property or all Index type
fn flush_group(result: &mut Vec<Stmt>, obj: Atom, accesses: Vec<AccessKind>) {
    if accesses.len() < 2 {
        // Not worth destructuring — emit individually
        for acc in accesses {
            result.push(acc_to_stmt(&obj, acc));
        }
        return;
    }
    // Check consistency: all property or all index
    let all_prop = accesses.iter().all(|a| matches!(a, AccessKind::Property { .. }));
    let all_idx = accesses.iter().all(|a| matches!(a, AccessKind::Index { .. }));

    if all_prop {
        flush_property_group(result, obj, accesses);
    } else if all_idx {
        flush_index_group(result, obj, accesses);
    } else {
        // Mixed — emit individually
        for acc in accesses {
            result.push(acc_to_stmt(&obj, acc));
        }
    }
}

fn flush_property_group(result: &mut Vec<Stmt>, obj: Atom, accesses: Vec<AccessKind>) {
    if accesses.len() < 2 {
        for acc in accesses {
            result.push(acc_to_stmt(&obj, acc));
        }
        return;
    }
    // Build ObjectPat
    let mut props: Vec<ObjectPatProp> = Vec::new();

    for acc in &accesses {
        let AccessKind::Property { binding, prop_key } = acc else { continue };
        let prop_name: PropName = match prop_key {
            PropKey::Ident(sym) => PropName::Ident(swc_core::ecma::ast::IdentName::new(sym.clone(), DUMMY_SP)),
            PropKey::Str(sym) => PropName::Str(swc_core::ecma::ast::Str {
                span: DUMMY_SP,
                value: sym.as_str().into(),
                raw: None,
            }),
        };

        let prop_sym = match prop_key {
            PropKey::Ident(s) => s.clone(),
            PropKey::Str(s) => s.clone(),
        };

        match binding {
            None => {
                // Standalone access: `obj.prop;` → include in destructuring without alias
                props.push(ObjectPatProp::Assign(swc_core::ecma::ast::AssignPatProp {
                    span: DUMMY_SP,
                    key: BindingIdent {
                        id: Ident::new_no_ctxt(prop_sym, DUMMY_SP),
                        type_ann: None,
                    },
                    value: None,
                }));
            }
            Some(alias) => {
                if alias == &prop_sym {
                    // Same name: shorthand
                    props.push(ObjectPatProp::Assign(swc_core::ecma::ast::AssignPatProp {
                        span: DUMMY_SP,
                        key: BindingIdent {
                            id: Ident::new_no_ctxt(alias.clone(), DUMMY_SP),
                            type_ann: None,
                        },
                        value: None,
                    }));
                } else {
                    // Different name: { key: alias }
                    props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                        key: prop_name,
                        value: Box::new(Pat::Ident(BindingIdent {
                            id: Ident::new_no_ctxt(alias.clone(), DUMMY_SP),
                            type_ann: None,
                        })),
                    }));
                }
            }
        }
    }

    result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Object(ObjectPat {
                span: DUMMY_SP,
                props,
                optional: false,
                type_ann: None,
            }),
            init: Some(Box::new(Expr::Ident(Ident::new_no_ctxt(obj, DUMMY_SP)))),
            definite: false,
        }],
    }))));
}

fn flush_index_group(result: &mut Vec<Stmt>, obj: Atom, accesses: Vec<AccessKind>) {
    if accesses.len() < 2 {
        for acc in accesses {
            result.push(acc_to_stmt(&obj, acc));
        }
        return;
    }
    // Find max index
    let max_idx = accesses
        .iter()
        .filter_map(|a| if let AccessKind::Index { index, .. } = a { Some(*index) } else { None })
        .max()
        .unwrap_or(0);

    // Build elems array with holes
    let mut elems: Vec<Option<Pat>> = vec![None; max_idx + 1];
    let non_inlined: Vec<Stmt> = Vec::new();

    for acc in &accesses {
        let AccessKind::Index { binding, index } = acc else { continue };
        if let Some(alias) = binding {
            elems[*index] = Some(Pat::Ident(BindingIdent {
                id: Ident::new_no_ctxt(alias.clone(), DUMMY_SP),
                type_ann: None,
            }));
        }
    }

    result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Array(ArrayPat {
                span: DUMMY_SP,
                elems,
                optional: false,
                type_ann: None,
            }),
            init: Some(Box::new(Expr::Ident(Ident::new_no_ctxt(obj, DUMMY_SP)))),
            definite: false,
        }],
    }))));

    result.extend(non_inlined);
}

fn acc_to_stmt(obj: &Atom, acc: AccessKind) -> Stmt {
    match acc {
        AccessKind::Property { binding, prop_key } => {
            let prop = match &prop_key {
                PropKey::Ident(s) => MemberProp::Ident(swc_core::ecma::ast::IdentName::new(s.clone(), DUMMY_SP)),
                PropKey::Str(s) => MemberProp::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: Box::new(Expr::Lit(Lit::Str(swc_core::ecma::ast::Str {
                        span: DUMMY_SP,
                        value: s.as_str().into(),
                        raw: None,
                    }))),
                }),
            };
            let member_expr = Expr::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Ident(Ident::new_no_ctxt(obj.clone(), DUMMY_SP))),
                prop,
            });
            match binding {
                None => Stmt::Expr(ExprStmt { span: DUMMY_SP, expr: Box::new(member_expr) }),
                Some(alias) => Stmt::Decl(Decl::Var(Box::new(VarDecl {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    kind: VarDeclKind::Const,
                    declare: false,
                    decls: vec![VarDeclarator {
                        span: DUMMY_SP,
                        name: Pat::Ident(BindingIdent {
                            id: Ident::new_no_ctxt(alias, DUMMY_SP),
                            type_ann: None,
                        }),
                        init: Some(Box::new(member_expr)),
                        definite: false,
                    }],
                }))),
            }
        }
        AccessKind::Index { binding, index } => {
            let member_expr = Expr::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Ident(Ident::new_no_ctxt(obj.clone(), DUMMY_SP))),
                prop: MemberProp::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: Box::new(Expr::Lit(Lit::Num(Number {
                        span: DUMMY_SP,
                        value: index as f64,
                        raw: None,
                    }))),
                }),
            });
            match binding {
                None => Stmt::Expr(ExprStmt { span: DUMMY_SP, expr: Box::new(member_expr) }),
                Some(alias) => Stmt::Decl(Decl::Var(Box::new(VarDecl {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    kind: VarDeclKind::Const,
                    declare: false,
                    decls: vec![VarDeclarator {
                        span: DUMMY_SP,
                        name: Pat::Ident(BindingIdent {
                            id: Ident::new_no_ctxt(alias, DUMMY_SP),
                            type_ann: None,
                        }),
                        init: Some(Box::new(member_expr)),
                        definite: false,
                    }],
                }))),
            }
        }
    }
}
