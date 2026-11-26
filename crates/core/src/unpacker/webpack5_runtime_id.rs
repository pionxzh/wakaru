//! Webpack 5 runtime-id conditions in a module shared by several entry
//! runtimes.
//!
//! When one module is reachable from several entry runtimes, webpack 5 can
//! emit code that tests the running runtime's chunk id
//! (`__webpack_require__.j`) inside the factory:
//!
//! ```js
//! if (23 == r.j) var a = r(582);
//! e.A = 23 == r.j ? class {} : null;
//! /^(23|695)$/.test(r.j);
//! ```
//!
//! Each entry runtime assigns its own id in its bootstrap
//! (`__webpack_require__.j = 23`). The split output has no runtime, so a kept
//! `require.j` reads `undefined` (or throws in ESM). This module reads the
//! runtime ids an input assigns, and folds a factory's conditions when every
//! runtime of the build present in the input gives the same result for every
//! condition: an `if` or `?:` keeps only the taken branch, and any other
//! condition becomes a boolean literal. A branch that no loaded runtime takes
//! is dropped on purpose; code for runtimes absent from the input is not kept.
//! No rule in the pipeline removes a constant `if (true)`, so the fold drops
//! the branch itself and the module's requires can become ESM imports.

use swc_core::atoms::Atom as SwcAtom;
use swc_core::common::util::take::Take;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, BinExpr, BinaryOp, BindingIdent, Bool, CallExpr, Callee,
    Class, Decl, Expr, ExprStmt, FnDecl, Function, Ident, Lit, MemberExpr, MemberProp, Module,
    ModuleItem, Pat, Stmt, UnaryExpr, UnaryOp, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::collections::HashSet;
use crate::utils::paren::strip_parens;

/// One runtime's chunk id, as its bootstrap assigns it to
/// `__webpack_require__.j`.
#[derive(Debug, Clone)]
pub(crate) enum WebpackRuntimeId {
    Number(f64),
    String(String),
    /// The runtime assigns a value that is not a number or string literal.
    /// No condition can be evaluated against it.
    NonLiteral,
}

impl PartialEq for WebpackRuntimeId {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) => left.to_bits() == right.to_bits(),
            (Self::String(left), Self::String(right)) => left == right,
            (Self::NonLiteral, Self::NonLiteral) => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for WebpackRuntimeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(value) => match js_number_to_string(*value) {
                Some(text) => write!(f, "`{text}`"),
                None => write!(f, "`{value}`"),
            },
            Self::String(value) => write!(f, "`{value:?}`"),
            Self::NonLiteral => write!(f, "a non-literal id"),
        }
    }
}

/// The runtime ids an input's webpack 5 bootstraps assign.
///
/// A bootstrap is the input's top level or the body of a top-level IIFE. Its
/// `__webpack_require__` is a function declared directly in that body that
/// also gets the module table assigned (`R.m = modules`, which webpack emits
/// whenever the runtime loads chunks, the only case with a `.j`). Assignments
/// `R.j = <value>` and `R.m = …` count in any expression position of that
/// body's statements, since a minifier merges the bootstrap's statements into
/// an `if` test, a `return` argument, a declarator, or a `for` header
/// (`if (u.m = o, u.n = …, …) {…}`). They do not count inside a nested
/// function, arrow, or class: a `.j` set in a callback is not the runtime's
/// id. Two different ids in one bootstrap are both recorded.
pub(crate) fn webpack_runtime_ids(module: &Module) -> Vec<WebpackRuntimeId> {
    let mut ids = Vec::new();
    let top_level = module
        .body
        .iter()
        .filter_map(|item| match item {
            ModuleItem::Stmt(stmt) => Some(stmt),
            ModuleItem::ModuleDecl(_) => None,
        })
        .collect::<Vec<_>>();
    collect_bootstrap_runtime_ids(&top_level, &mut ids);
    for stmt in &top_level {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            continue;
        };
        if let Some(body) = super::webpack5::extract_iife_body(expr) {
            collect_bootstrap_runtime_ids(&body.stmts.iter().collect::<Vec<_>>(), &mut ids);
        }
    }
    ids
}

/// The runtime of an input that no detector splits: a webpack 5 runtime-only
/// file (`optimization.runtimeChunk`), whose own module table is empty, so
/// no chunk detection matches it. Its ids still decide the conditions of the
/// modules its build's chunks hold.
#[derive(Debug, Clone)]
pub(crate) struct UndetectedWebpackRuntime {
    /// The chunk-loading globals the runtime binds (its build identity, as
    /// [`super::DetectedBundle::chunk_loading_globals`] reads it).
    pub(crate) chunk_loading_globals: Vec<String>,
    pub(crate) runtime_ids: Vec<WebpackRuntimeId>,
}

/// [`UndetectedWebpackRuntime`] for an input no detector split, when its
/// bootstraps assign a runtime id.
pub(crate) fn undetected_webpack_runtime(module: &Module) -> Option<UndetectedWebpackRuntime> {
    let runtime_ids = webpack_runtime_ids(module);
    if runtime_ids.is_empty() {
        return None;
    }
    Some(UndetectedWebpackRuntime {
        chunk_loading_globals: super::chunk_loading_global::chunk_loading_globals(module),
        runtime_ids,
    })
}

fn collect_bootstrap_runtime_ids(stmts: &[&Stmt], ids: &mut Vec<WebpackRuntimeId>) {
    let declared_functions = stmts
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Decl(Decl::Fn(decl)) => Some(decl.ident.sym.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    if declared_functions.is_empty() {
        return;
    }
    let mut collector = BootstrapAssignments {
        declared_functions: &declared_functions,
        table_owners: HashSet::default(),
        assigned: Vec::new(),
    };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    for (owner, id) in collector.assigned {
        if collector.table_owners.contains(&owner) && !ids.contains(&id) {
            ids.push(id);
        }
    }
}

/// `R.m = …` and `R.j = …` on a declared function `R` in the bootstrap's own
/// code: not inside a nested function (a declared function's body included),
/// arrow, or class.
struct BootstrapAssignments<'a> {
    declared_functions: &'a HashSet<SwcAtom>,
    table_owners: HashSet<SwcAtom>,
    assigned: Vec<(SwcAtom, WebpackRuntimeId)>,
}

impl Visit for BootstrapAssignments<'_> {
    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_class(&mut self, _: &Class) {}

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        assign.visit_children_with(self);
        if assign.op != AssignOp::Assign {
            return;
        }
        let Some(member) = assign.left.as_simple().and_then(|left| left.as_member()) else {
            return;
        };
        let Expr::Ident(object) = &*member.obj else {
            return;
        };
        if !self.declared_functions.contains(&object.sym) {
            return;
        }
        let MemberProp::Ident(prop) = &member.prop else {
            return;
        };
        match prop.sym.as_ref() {
            "m" => {
                self.table_owners.insert(object.sym.clone());
            }
            "j" => {
                let id = match strip_parens(&assign.right) {
                    Expr::Lit(Lit::Num(number)) => WebpackRuntimeId::Number(number.value),
                    Expr::Lit(Lit::Str(string)) => match string.value.as_str() {
                        Some(value) => WebpackRuntimeId::String(value.to_string()),
                        None => WebpackRuntimeId::NonLiteral,
                    },
                    _ => WebpackRuntimeId::NonLiteral,
                };
                self.assigned.push((object.sym.clone(), id));
            }
            _ => {}
        }
    }
}

/// Why a module's runtime-id conditions stay as they are.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RuntimeIdFoldRefusal {
    /// No runtime of the module's build in the input assigns an id.
    NoRuntime,
    /// The runtimes give different results, or one cannot be evaluated.
    Disagree(Vec<WebpackRuntimeId>),
}

/// Fold the runtime-id conditions of a normalized webpack 5 factory.
///
/// The factory's loader parameter has already been renamed to the free
/// `require` by extraction, and that rename refuses a factory where a free
/// `require` is used other than as a call. A `.j` read on the unresolved
/// `require` is therefore a read of the loader parameter's binding; a
/// shadowing local `require` has a resolved context and is left alone.
///
/// Returns `Ok(true)` when conditions were folded, `Ok(false)` when the
/// module has none, and the refusal otherwise (the module is untouched).
pub(crate) fn fold_runtime_id_conditions(
    module: &mut Module,
    unresolved_mark: Mark,
    runtime_ids: &[WebpackRuntimeId],
) -> Result<bool, RuntimeIdFoldRefusal> {
    let mut check = ConditionCheck {
        unresolved_mark,
        runtime_ids,
        found: false,
        agrees: true,
    };
    module.visit_with(&mut check);
    if !check.found {
        return Ok(false);
    }
    if runtime_ids.is_empty() {
        return Err(RuntimeIdFoldRefusal::NoRuntime);
    }
    if !check.agrees {
        return Err(RuntimeIdFoldRefusal::Disagree(runtime_ids.to_vec()));
    }
    module.visit_mut_with(&mut ConditionFolder {
        unresolved_mark,
        runtime_ids,
    });
    Ok(true)
}

struct ConditionCheck<'a> {
    unresolved_mark: Mark,
    runtime_ids: &'a [WebpackRuntimeId],
    found: bool,
    agrees: bool,
}

impl Visit for ConditionCheck<'_> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Some(condition) = RuntimeIdCondition::from_expr(expr, self.unresolved_mark) {
            self.found = true;
            if condition.uniform_result(self.runtime_ids).is_none() {
                self.agrees = false;
            }
            return;
        }
        expr.visit_children_with(self);
    }
}

struct ConditionFolder<'a> {
    unresolved_mark: Mark,
    runtime_ids: &'a [WebpackRuntimeId],
}

impl ConditionFolder<'_> {
    /// The folded value of a runtime-id condition, or of its negation.
    fn folded_value(&self, expr: &Expr) -> Option<bool> {
        let expr = strip_parens(expr);
        if let Expr::Unary(UnaryExpr {
            op: UnaryOp::Bang,
            arg,
            ..
        }) = expr
        {
            return self.folded_value(arg).map(|value| !value);
        }
        RuntimeIdCondition::from_expr(expr, self.unresolved_mark)?.uniform_result(self.runtime_ids)
    }

    /// The statements that replace an `if` whose test is a folded condition:
    /// the taken branch, plus a bare `var` for each name the dropped branch
    /// declares with `var` (it is hoisted, so the binding must stay). A
    /// dropped branch that declares a function keeps the `if`.
    fn fold_if(&self, stmt: &mut Stmt) -> Option<Vec<Stmt>> {
        let Stmt::If(if_stmt) = stmt else {
            return None;
        };
        let value = self.folded_value(&if_stmt.test)?;
        let (taken, dropped) = if value {
            (Some(&mut if_stmt.cons), if_stmt.alt.as_mut())
        } else {
            (if_stmt.alt.as_mut(), Some(&mut if_stmt.cons))
        };
        let mut hoisted = HoistedVarNames::default();
        if let Some(dropped) = &dropped {
            dropped.visit_with(&mut hoisted);
        }
        if hoisted.declares_function {
            return None;
        }
        let mut replacement = Vec::new();
        if let Some(taken) = taken {
            let taken = (**taken).take();
            match taken {
                Stmt::Block(block) if block.stmts.iter().all(is_spliceable_stmt) => {
                    replacement.extend(block.stmts);
                }
                Stmt::Empty(_) => {}
                taken => replacement.push(taken),
            }
        }
        if !hoisted.names.is_empty() {
            replacement.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                span: DUMMY_SP,
                ctxt: Default::default(),
                kind: VarDeclKind::Var,
                declare: false,
                decls: hoisted
                    .names
                    .into_iter()
                    .map(|name| VarDeclarator {
                        span: DUMMY_SP,
                        name: Pat::Ident(BindingIdent::from(name)),
                        init: None,
                        definite: false,
                    })
                    .collect(),
            }))));
        }
        Some(replacement)
    }

    /// Push `stmt`, or what its folded `if` leaves, onto `out`. The taken
    /// branch may hold further conditions, so its statements fold again.
    fn fold_into(&mut self, mut stmt: Stmt, out: &mut Vec<Stmt>) {
        match self.fold_if(&mut stmt) {
            Some(replacement) => {
                for stmt in replacement {
                    self.fold_into(stmt, out);
                }
            }
            None => {
                stmt.visit_mut_with(self);
                out.push(stmt);
            }
        }
    }
}

/// A statement that keeps its meaning when moved out of its block: not a
/// block-scoped or function declaration.
fn is_spliceable_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Decl(Decl::Var(var)) => var.kind == VarDeclKind::Var,
        Stmt::Decl(_) => false,
        _ => true,
    }
}

/// `var` names a statement declares, without entering nested functions.
#[derive(Default)]
struct HoistedVarNames {
    names: Vec<Ident>,
    declares_function: bool,
}

impl Visit for HoistedVarNames {
    fn visit_var_decl(&mut self, var: &VarDecl) {
        if var.kind == VarDeclKind::Var {
            for declarator in &var.decls {
                let ids: Vec<Ident> = find_pat_ids(&declarator.name);
                for id in ids {
                    if !self
                        .names
                        .iter()
                        .any(|known| known.sym == id.sym && known.ctxt == id.ctxt)
                    {
                        self.names.push(id);
                    }
                }
            }
        }
        var.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, _: &FnDecl) {
        self.declares_function = true;
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_class(&mut self, _: &Class) {}
}

impl VisitMut for ConditionFolder<'_> {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        let mut folded = Vec::with_capacity(items.len());
        for item in items.drain(..) {
            match item {
                ModuleItem::Stmt(stmt) => {
                    let mut stmts = Vec::new();
                    self.fold_into(stmt, &mut stmts);
                    folded.extend(stmts.into_iter().map(ModuleItem::Stmt));
                }
                mut item => {
                    item.visit_mut_with(self);
                    folded.push(item);
                }
            }
        }
        *items = folded;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        let mut folded = Vec::with_capacity(stmts.len());
        for stmt in stmts.drain(..) {
            self.fold_into(stmt, &mut folded);
        }
        *stmts = folded;
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Cond(cond) = expr {
            if let Some(value) = self.folded_value(&cond.test) {
                let taken = if value { &mut cond.cons } else { &mut cond.alt };
                *expr = *taken.take();
                self.visit_mut_expr(expr);
                return;
            }
        }
        if let Some(value) = self.folded_value(expr) {
            *expr = Expr::Lit(Lit::Bool(Bool {
                span: DUMMY_SP,
                value,
            }));
            return;
        }
        expr.visit_mut_children_with(self);
    }
}

/// A recognized test of the runtime id.
enum RuntimeIdCondition<'a> {
    /// `lit == require.j` and the `===`, `!=`, `!==` forms, either side.
    Compare { op: BinaryOp, literal: &'a Lit },
    /// `/^(...)$/.test(require.j)`.
    RegexTest(Pattern),
}

impl<'a> RuntimeIdCondition<'a> {
    fn from_expr(expr: &'a Expr, unresolved_mark: Mark) -> Option<Self> {
        match expr {
            Expr::Bin(BinExpr {
                op, left, right, ..
            }) if matches!(
                op,
                BinaryOp::EqEq | BinaryOp::EqEqEq | BinaryOp::NotEq | BinaryOp::NotEqEq
            ) =>
            {
                let literal = if is_runtime_id_read(right, unresolved_mark) {
                    left
                } else if is_runtime_id_read(left, unresolved_mark) {
                    right
                } else {
                    return None;
                };
                let Expr::Lit(literal @ (Lit::Num(_) | Lit::Str(_))) = strip_parens(literal) else {
                    return None;
                };
                Some(Self::Compare { op: *op, literal })
            }
            Expr::Call(call) => regex_test_pattern(call, unresolved_mark).map(Self::RegexTest),
            _ => None,
        }
    }

    /// The result every runtime gives, or `None` when they differ or one
    /// cannot be evaluated.
    fn uniform_result(&self, runtime_ids: &[WebpackRuntimeId]) -> Option<bool> {
        let mut result = None;
        for id in runtime_ids {
            let value = self.evaluate(id)?;
            match result {
                None => result = Some(value),
                Some(previous) if previous != value => return None,
                Some(_) => {}
            }
        }
        result
    }

    fn evaluate(&self, id: &WebpackRuntimeId) -> Option<bool> {
        match self {
            Self::Compare { op, literal } => {
                let equal = match op {
                    BinaryOp::EqEq | BinaryOp::NotEq => loose_equals(literal, id)?,
                    _ => strict_equals(literal, id)?,
                };
                Some(match op {
                    BinaryOp::EqEq | BinaryOp::EqEqEq => equal,
                    _ => !equal,
                })
            }
            Self::RegexTest(pattern) => {
                let text = match id {
                    WebpackRuntimeId::Number(value) => js_number_to_string(*value)?,
                    WebpackRuntimeId::String(value) => value.clone(),
                    WebpackRuntimeId::NonLiteral => return None,
                };
                Some(pattern.full_match(&text))
            }
        }
    }
}

fn is_runtime_id_read(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(expr) else {
        return false;
    };
    let Expr::Ident(object) = strip_parens(obj) else {
        return false;
    };
    object.sym.as_ref() == "require"
        && object.ctxt.outer() == unresolved_mark
        && matches!(prop, MemberProp::Ident(name) if name.sym.as_ref() == "j")
}

fn regex_test_pattern(call: &CallExpr, unresolved_mark: Mark) -> Option<Pattern> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee) else {
        return None;
    };
    if !matches!(prop, MemberProp::Ident(name) if name.sym.as_ref() == "test") {
        return None;
    }
    let Expr::Lit(Lit::Regex(regex)) = strip_parens(obj) else {
        return None;
    };
    // Flags change matching (`i`, `u`, ...) or make `test` stateful (`g`, `y`).
    if !regex.flags.is_empty() {
        return None;
    }
    let [argument] = call.args.as_slice() else {
        return None;
    };
    if argument.spread.is_some() || !is_runtime_id_read(&argument.expr, unresolved_mark) {
        return None;
    }
    Pattern::parse(regex.exp.as_ref())
}

fn strict_equals(literal: &Lit, id: &WebpackRuntimeId) -> Option<bool> {
    match (literal, id) {
        (_, WebpackRuntimeId::NonLiteral) => None,
        (Lit::Num(number), WebpackRuntimeId::Number(value)) => Some(number.value == *value),
        (Lit::Str(string), WebpackRuntimeId::String(value)) => {
            Some(string.value.as_str()? == value.as_str())
        }
        (Lit::Num(_), WebpackRuntimeId::String(_)) | (Lit::Str(_), WebpackRuntimeId::Number(_)) => {
            Some(false)
        }
        _ => None,
    }
}

/// `==` between a number or string literal and a runtime id: a number and a
/// string compare after the string's `ToNumber`.
fn loose_equals(literal: &Lit, id: &WebpackRuntimeId) -> Option<bool> {
    match (literal, id) {
        (Lit::Num(number), WebpackRuntimeId::String(value)) => {
            Some(js_string_to_number(value)? == number.value)
        }
        (Lit::Str(string), WebpackRuntimeId::Number(value)) => {
            Some(js_string_to_number(string.value.as_str()?)? == *value)
        }
        _ => strict_equals(literal, id),
    }
}

/// JavaScript's `ToNumber` on a string, for ASCII input. Returns `None` for
/// input outside that subset so the caller does not guess. `NaN` is returned
/// as `f64::NAN`, which equals nothing.
fn js_string_to_number(value: &str) -> Option<f64> {
    if !value.is_ascii() {
        return None;
    }
    let trimmed =
        value.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c'));
    if trimmed.is_empty() {
        return Some(0.0);
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = trimmed.strip_prefix(prefix) {
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return Some(f64::NAN);
            }
            return Some(digits.chars().fold(0.0, |acc, c| {
                acc * radix as f64 + c.to_digit(radix).unwrap() as f64
            }));
        }
    }
    match trimmed {
        "Infinity" | "+Infinity" => return Some(f64::INFINITY),
        "-Infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    // Rust also accepts `inf` and `nan`; JavaScript accepts only digits,
    // a sign, a point, and an exponent here.
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E'))
        || !trimmed.chars().any(|c| c.is_ascii_digit())
    {
        return Some(f64::NAN);
    }
    Some(trimmed.parse::<f64>().unwrap_or(f64::NAN))
}

/// JavaScript's `ToString` for the integral numbers webpack uses as chunk
/// ids. Other numbers return `None`.
fn js_number_to_string(value: f64) -> Option<String> {
    if !value.is_finite() || value.fract() != 0.0 || value.abs() >= 1e21 {
        return None;
    }
    if value == 0.0 {
        return Some("0".to_string());
    }
    Some(format!("{value:.0}"))
}

/// The `^(...)$` alternation grammar webpack emits for a set of runtime ids:
/// literal characters (escaped metacharacters included), groups, `|`, and
/// character classes of literal characters. Anything else is not parsed, so
/// the condition is left as is.
#[derive(Debug)]
struct Pattern(Alternation);

#[derive(Debug)]
struct Alternation(Vec<Vec<Atom>>);

#[derive(Debug)]
enum Atom {
    Char(char),
    Class(Vec<char>),
    Group(Alternation),
}

impl Pattern {
    fn parse(source: &str) -> Option<Self> {
        let inner = source.strip_prefix('^')?.strip_suffix('$')?;
        // A trailing `\$` is an escaped dollar, not the anchor.
        if inner.ends_with('\\') {
            return None;
        }
        let chars = inner.chars().collect::<Vec<_>>();
        let mut pos = 0;
        let alternation = parse_alternation(&chars, &mut pos)?;
        (pos == chars.len()).then_some(Self(alternation))
    }

    fn full_match(&self, text: &str) -> bool {
        let chars = text.chars().collect::<Vec<_>>();
        match_alternation(&self.0, &chars, 0).contains(&chars.len())
    }
}

fn parse_alternation(chars: &[char], pos: &mut usize) -> Option<Alternation> {
    let mut branches = vec![Vec::new()];
    while let Some(&c) = chars.get(*pos) {
        match c {
            '|' => {
                *pos += 1;
                branches.push(Vec::new());
            }
            ')' => break,
            '(' => {
                *pos += 1;
                if chars.get(*pos) == Some(&'?') {
                    if chars.get(*pos + 1) != Some(&':') {
                        return None;
                    }
                    *pos += 2;
                }
                let group = parse_alternation(chars, pos)?;
                if chars.get(*pos) != Some(&')') {
                    return None;
                }
                *pos += 1;
                branches.last_mut()?.push(Atom::Group(group));
            }
            '[' => {
                *pos += 1;
                let mut members = Vec::new();
                loop {
                    match *chars.get(*pos)? {
                        ']' => {
                            *pos += 1;
                            break;
                        }
                        '\\' => {
                            members.push(escaped_literal(*chars.get(*pos + 1)?)?);
                            *pos += 2;
                        }
                        // A leading `^` negates, and `-` forms a range.
                        '^' | '-' | '[' => return None,
                        c => {
                            members.push(c);
                            *pos += 1;
                        }
                    }
                }
                if members.is_empty() {
                    return None;
                }
                branches.last_mut()?.push(Atom::Class(members));
            }
            '\\' => {
                let escaped = escaped_literal(*chars.get(*pos + 1)?)?;
                *pos += 2;
                branches.last_mut()?.push(Atom::Char(escaped));
            }
            '.' | '*' | '+' | '?' | '{' | '}' | '^' | '$' | ']' => return None,
            c => {
                *pos += 1;
                branches.last_mut()?.push(Atom::Char(c));
            }
        }
    }
    Some(Alternation(branches))
}

/// An escape that stands for its own character. Class escapes such as `\d`
/// or `\w`, and back-references, are not literals.
fn escaped_literal(c: char) -> Option<char> {
    (!c.is_ascii_alphanumeric()).then_some(c)
}

/// Every end position the alternation can reach from `start`.
fn match_alternation(alternation: &Alternation, text: &[char], start: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    for branch in &alternation.0 {
        let mut positions = vec![start];
        for atom in branch {
            let mut next = Vec::new();
            for &position in &positions {
                match atom {
                    Atom::Char(c) => {
                        if text.get(position) == Some(c) {
                            next.push(position + 1);
                        }
                    }
                    Atom::Class(members) => {
                        if text.get(position).is_some_and(|c| members.contains(c)) {
                            next.push(position + 1);
                        }
                    }
                    Atom::Group(group) => next.extend(match_alternation(group, text, position)),
                }
            }
            next.sort_unstable();
            next.dedup();
            positions = next;
            if positions.is_empty() {
                break;
            }
        }
        ends.extend(positions);
    }
    ends.sort_unstable();
    ends.dedup();
    ends
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, text: &str) -> Option<bool> {
        Pattern::parse(pattern).map(|pattern| pattern.full_match(text))
    }

    fn ids(source: &str) -> Vec<WebpackRuntimeId> {
        swc_core::common::GLOBALS.set(&Default::default(), || {
            let cm: swc_core::common::sync::Lrc<swc_core::common::SourceMap> = Default::default();
            let module = super::super::parse_es_module(source, "input.js", cm)
                .expect("fixture should parse");
            webpack_runtime_ids(&module)
        })
    }

    #[test]
    fn runtime_ids_are_read_in_any_bootstrap_expression_position() {
        let number = |value| vec![WebpackRuntimeId::Number(value)];
        // Merged into an `if` test, a `return` argument, a declarator, and a
        // `for` header.
        assert_eq!(
            ids("(()=>{var o={};function u(e){}if(u.m=o,u.n=1,u.j=7){}})()"),
            number(7.0)
        );
        assert_eq!(
            ids("(function(){var o={};function u(e){}return u.m=o,u.j=7,u(1)})()"),
            number(7.0)
        );
        assert_eq!(
            ids("(()=>{var o={};function u(e){}var a=(u.m=o,u.j=7);})()"),
            number(7.0)
        );
        assert_eq!(
            ids("(()=>{var o={};function u(e){}for(u.m=o,u.j=\"main\";;)break})()"),
            vec![WebpackRuntimeId::String("main".into())]
        );
        // Two ids in one bootstrap are both kept.
        assert_eq!(
            ids("(()=>{var o={};function u(e){}u.m=o;if(u.j=7){}u.j=8})()"),
            vec![WebpackRuntimeId::Number(7.0), WebpackRuntimeId::Number(8.0)]
        );
    }

    #[test]
    fn runtime_ids_in_nested_functions_do_not_count() {
        // Not the runtime's own `.j`: a callback, a function body, a class.
        assert_eq!(
            ids("(()=>{var o={};function u(e){u.j=9}u.m=o,u.j=7,(()=>{u.j=8})(),setTimeout(function(){u.j=6}),class{x(){u.j=5}}})()"),
            vec![WebpackRuntimeId::Number(7.0)]
        );
        // `u.m` set only in a callback does not make `u` the runtime.
        assert!(ids("(()=>{var o={};function u(e){}(()=>{u.m=o})(),u.j=7})()").is_empty());
    }

    #[test]
    fn regex_alternation_follows_regex_semantics() {
        assert_eq!(matches("^(23|695)$", "23"), Some(true));
        assert_eq!(matches("^(23|695)$", "695"), Some(true));
        assert_eq!(matches("^(23|695)$", "2"), Some(false));
        assert_eq!(matches("^(23|695)$", "2369"), Some(false));
        let nested = "^(1(143|92)|2(341|723))$";
        for id in ["1143", "192", "2341", "2723"] {
            assert_eq!(matches(nested, id), Some(true), "{id}");
        }
        for id in ["1", "143", "1341", "2192", "11143"] {
            assert_eq!(matches(nested, id), Some(false), "{id}");
        }
        assert_eq!(matches("^(12(|3))$", "12"), Some(true));
        assert_eq!(matches("^(12(|3))$", "123"), Some(true));
        assert_eq!(matches("^([135]|24)$", "3"), Some(true));
        assert_eq!(matches("^([135]|24)$", "2"), Some(false));
        assert_eq!(matches("^(main|app\\-b)$", "app-b"), Some(true));
        assert_eq!(matches("^(?:a|b)$", "b"), Some(true));
    }

    #[test]
    fn regex_outside_the_webpack_grammar_is_not_parsed() {
        for pattern in [
            "(23|695)",
            "^(23|695)",
            "^\\d+$",
            "^2.$",
            "^(2|3)+$",
            "^[1-3]$",
            "^[^1]$",
            "^(?=2)23$",
            "^(23|695$",
            "^23\\$",
        ] {
            assert!(Pattern::parse(pattern).is_none(), "{pattern}");
        }
    }

    #[test]
    fn string_to_number_follows_javascript() {
        assert_eq!(js_string_to_number("23"), Some(23.0));
        assert_eq!(js_string_to_number(" 23 "), Some(23.0));
        assert_eq!(js_string_to_number(""), Some(0.0));
        assert_eq!(js_string_to_number("0x17"), Some(23.0));
        assert_eq!(js_string_to_number("2.3e1"), Some(23.0));
        assert!(js_string_to_number("inf").unwrap().is_nan());
        assert!(js_string_to_number("main").unwrap().is_nan());
        assert!(js_string_to_number("0x").unwrap().is_nan());
        assert_eq!(js_string_to_number("\u{a0}23"), None);
    }
}
