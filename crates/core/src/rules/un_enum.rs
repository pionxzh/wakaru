use std::collections::{HashSet, VecDeque};

use swc_core::atoms::{Atom, Wtf8Atom};
use swc_core::common::{Mark, Span, Spanned, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BindingIdent, BlockStmtOrExpr, CallExpr,
    Callee, ComputedPropName, Decl, ExportNamedSpecifier, ExportSpecifier, Expr, ExprStmt, FnExpr,
    Ident, IdentName, KeyValueProp, Lit, MemberExpr, MemberProp, ModuleDecl, ModuleExportName,
    ModuleItem, NamedExport, Number, ObjectLit, Pat, Prop, PropName, PropOrSpread,
    SimpleAssignTarget, Stmt, Str, UnaryExpr, UnaryOp, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::decl_utils::collect_decl_names;
use crate::utils::paren::strip_parens;

pub struct UnEnum {
    unresolved_mark: Option<Mark>,
}

impl UnEnum {
    pub fn new() -> Self {
        Self {
            unresolved_mark: None,
        }
    }

    pub fn new_with_mark(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_mark: Some(unresolved_mark),
        }
    }
}

impl Default for UnEnum {
    fn default() -> Self {
        Self::new()
    }
}

impl VisitMut for UnEnum {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        process_module_items_for_enum(items, self.unresolved_mark);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        process_stmts_for_enum(stmts);
    }
}

// ============================================================
// Data structures
// ============================================================

struct EnumMember {
    /// The forward key (string)
    key: EnumKey,
    /// The value expression
    value: Box<Expr>,
    /// For numeric values: the reverse mapping (numeric_key_expr, string_name_expr)
    reverse: Option<(Box<Expr>, Box<Expr>)>,
}

enum EnumKey {
    /// Valid JS identifier → use IdentName key
    Ident(Atom),
    /// Invalid identifier (e.g. "2D") → use Str key
    Str(Wtf8Atom),
}

// ============================================================
// Processing logic
// ============================================================

fn process_module_items_for_enum(items: &mut Vec<ModuleItem>, unresolved_mark: Option<Mark>) {
    let mut exported_names = collect_exported_names(items);
    let mut remaining: VecDeque<ModuleItem> = std::mem::take(items).into();

    while let Some(item) = remaining.pop_front() {
        match item {
            ModuleItem::Stmt(stmt) => {
                let mut stmt = stmt;
                if rewrite_enum_var_decl_stmt(&mut stmt) {
                    items.push(ModuleItem::Stmt(stmt));
                    continue;
                }

                // Check if this is a bare var decl like `var Direction;`
                if let Some(bare_var_ident) = get_bare_var_decl_ident(&stmt) {
                    if let Some(ModuleItem::Stmt(next_stmt)) = remaining.front() {
                        if let Some(members) = parse_enum_iife(next_stmt, &bare_var_ident) {
                            // Consume the IIFE statement
                            remaining.pop_front();
                            let new_stmt = build_enum_var_decl(&bare_var_ident, members, &stmt);
                            items.push(ModuleItem::Stmt(new_stmt));
                            continue;
                        }

                        if let Some((public_name, members)) = unresolved_mark
                            .and_then(|mark| {
                                parse_exported_enum_iife(next_stmt, &bare_var_ident, mark)
                            })
                            .filter(|(public_name, _)| !exported_names.contains(public_name))
                            .filter(|(public_name, _)| {
                                !module_items_reference_public_export(
                                    remaining.iter().skip(1),
                                    public_name,
                                    unresolved_mark.expect("exported enum parsing requires a mark"),
                                )
                            })
                        {
                            remaining.pop_front();
                            let new_stmt = build_enum_var_decl(&bare_var_ident, members, &stmt);
                            items.push(ModuleItem::Stmt(new_stmt));
                            exported_names.insert(public_name.clone());
                            items.push(build_named_enum_export(&bare_var_ident, public_name));
                            continue;
                        }
                    }
                }

                // Try standalone enum IIFE (without preceding bare var)
                if let Some((ident, members)) = parse_enum_iife_standalone(&stmt) {
                    let new_stmt = build_enum_assign_stmt(ident, members, stmt.span());
                    items.push(ModuleItem::Stmt(new_stmt));
                    continue;
                }

                if let Some((local_ident, public_name, members)) = unresolved_mark
                    .and_then(|mark| parse_exported_enum_iife_standalone(&stmt, mark))
                    .filter(|(_, public_name, _)| !exported_names.contains(public_name))
                    .filter(|(local_ident, public_name, _)| {
                        has_safe_prior_bare_var(
                            items,
                            local_ident,
                            public_name,
                            unresolved_mark.expect("exported enum parsing requires a mark"),
                        )
                    })
                    .filter(|(_, public_name, _)| {
                        !module_items_reference_public_export(
                            remaining.iter(),
                            public_name,
                            unresolved_mark.expect("exported enum parsing requires a mark"),
                        )
                    })
                {
                    let new_stmt =
                        build_enum_assign_stmt(local_ident.clone(), members, stmt.span());
                    items.push(ModuleItem::Stmt(new_stmt));
                    exported_names.insert(public_name.clone());
                    items.push(build_named_enum_export(&local_ident, public_name));
                    continue;
                }

                items.push(ModuleItem::Stmt(stmt));
            }
            ModuleItem::ModuleDecl(mut module_decl) => {
                if rewrite_enum_export_decl(&mut module_decl) {
                    items.push(ModuleItem::ModuleDecl(module_decl));
                    continue;
                }

                items.push(ModuleItem::ModuleDecl(module_decl));
            }
        }
    }
}

fn module_items_reference_public_export<'a>(
    items: impl IntoIterator<Item = &'a ModuleItem>,
    public_name: &Atom,
    unresolved_mark: Mark,
) -> bool {
    items.into_iter().any(|item| {
        let mut finder = PublicExportUseFinder {
            public_name,
            unresolved_mark,
            found: false,
        };
        item.visit_with(&mut finder);
        finder.found
    })
}

struct PublicExportUseFinder<'a> {
    public_name: &'a Atom,
    unresolved_mark: Mark,
    found: bool,
}

impl Visit for PublicExportUseFinder<'_> {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        self.found |= unresolved_exports_member(member, self.unresolved_mark).as_ref()
            == Some(self.public_name);
        if !self.found {
            member.visit_children_with(self);
        }
    }
}

fn process_stmts_for_enum(stmts: &mut Vec<Stmt>) {
    let old: Vec<Stmt> = std::mem::take(stmts);
    let mut iter = old.into_iter().peekable();

    while let Some(stmt) = iter.next() {
        let mut stmt = stmt;
        if rewrite_enum_var_decl_stmt(&mut stmt) {
            stmts.push(stmt);
            continue;
        }

        if let Some(bare_var_ident) = get_bare_var_decl_ident(&stmt) {
            if let Some(peeked) = iter.peek() {
                if let Some(members) = parse_enum_iife(peeked, &bare_var_ident) {
                    iter.next(); // consume the IIFE
                    let new_stmt = build_enum_var_decl(&bare_var_ident, members, &stmt);
                    stmts.push(new_stmt);
                    continue;
                }
            }
        }

        if let Some((ident, members)) = parse_enum_iife_standalone(&stmt) {
            let new_stmt = build_enum_assign_stmt(ident, members, stmt.span());
            stmts.push(new_stmt);
            continue;
        }

        stmts.push(stmt);
    }
}

fn rewrite_enum_var_decl_stmt(stmt: &mut Stmt) -> bool {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return false;
    };
    rewrite_enum_var_decl(var)
}

fn rewrite_enum_export_decl(module_decl: &mut ModuleDecl) -> bool {
    let ModuleDecl::ExportDecl(export_decl) = module_decl else {
        return false;
    };
    let Decl::Var(var) = &mut export_decl.decl else {
        return false;
    };
    rewrite_enum_var_decl(var)
}

fn rewrite_enum_var_decl(var: &mut VarDecl) -> bool {
    let mut changed = false;

    for declarator in &mut var.decls {
        let Pat::Ident(BindingIdent { id, .. }) = &declarator.name else {
            continue;
        };
        let Some(init) = &mut declarator.init else {
            continue;
        };
        let Some(members) = parse_enum_iife_expr(init, Some(id)) else {
            continue;
        };
        **init = build_enum_object(members);
        changed = true;
    }

    changed
}

// ============================================================
// Detection helpers
// ============================================================

/// Check if stmt is `var Name;` (VarDecl with 1 declarator, no init)
fn get_bare_var_decl_ident(stmt: &Stmt) -> Option<Ident> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let declarator = &var.decls[0];
    if declarator.init.is_some() {
        return None;
    }
    let Pat::Ident(BindingIdent { id, .. }) = &declarator.name else {
        return None;
    };
    Some(id.clone())
}

/// Parse an enum IIFE where the inner function param name matches `expected_name`.
/// Also handles mangled enums where the param name differs from `expected_name`
/// (the arg `expected_name || (expected_name = {})` determines the enum name).
fn parse_enum_iife(stmt: &Stmt, expected_ident: &Ident) -> Option<Vec<EnumMember>> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    parse_enum_iife_expr(expr, Some(expected_ident))
}

/// Parse the TypeScript CommonJS form:
///
/// `var Local; (function (e) { ... })(Local = exports.Public || (exports.Public = {}));`
///
/// A second emitted form, `Local || (exports.Public = Local = {})`, is also
/// accepted. The exported variant is intentionally stricter than local enum
/// recovery: the body may contain only literal enum values, so replacing the
/// early CommonJS publication with an ESM binding cannot hide observable work.
fn parse_exported_enum_iife(
    stmt: &Stmt,
    local_ident: &Ident,
    unresolved_mark: Mark,
) -> Option<(Atom, Vec<EnumMember>)> {
    let (parsed_local, public_name, members) =
        parse_exported_enum_iife_standalone(stmt, unresolved_mark)?;
    same_binding(&parsed_local, local_ident).then_some((public_name, members))
}

fn parse_exported_enum_iife_standalone(
    stmt: &Stmt,
    unresolved_mark: Mark,
) -> Option<(Ident, Atom, Vec<EnumMember>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = strip_unary_bang(expr) else {
        return None;
    };
    if call.args.len() != 1 {
        return None;
    }
    let (local_ident, public_name) = parse_exported_enum_arg(&call.args[0].expr, unresolved_mark)?;

    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let members = if let Some((param, body)) = extract_enum_iife_expr_body(callee) {
        parse_enum_expr_body(body, param)?
    } else {
        let (param, body) = extract_enum_iife_body(callee)?;
        parse_enum_body(body, param)?
    };
    if !members.iter().all(enum_member_is_literal_only) {
        return None;
    }

    Some((local_ident, public_name, members))
}

fn parse_exported_enum_arg(expr: &Expr, unresolved_mark: Mark) -> Option<(Ident, Atom)> {
    let expr = strip_parens(expr);

    // `Local = exports.Public || (exports.Public = {})`
    if let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = expr
    {
        let AssignTarget::Simple(SimpleAssignTarget::Ident(left_ident)) = left else {
            return None;
        };
        let local_ident = left_ident.id.clone();
        let Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) = strip_parens(right)
        else {
            return None;
        };
        let Expr::Member(left_member) = strip_parens(left) else {
            return None;
        };
        let public_name = unresolved_exports_member(left_member, unresolved_mark)?;
        if assign_member_empty_object(right, &public_name, unresolved_mark) {
            return Some((local_ident, public_name));
        }
        return None;
    }

    // `Local || (exports.Public = Local = {})`
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left,
        right,
        ..
    }) = expr
    else {
        return None;
    };
    let Expr::Ident(left_ident) = strip_parens(left) else {
        return None;
    };
    let local_ident = left_ident.clone();
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = strip_parens(right)
    else {
        return None;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(export_member)) = left else {
        return None;
    };
    let public_name = unresolved_exports_member(export_member, unresolved_mark)?;
    if is_assign_empty_obj(right, &local_ident) {
        Some((local_ident, public_name))
    } else {
        None
    }
}

fn assign_member_empty_object(expr: &Expr, public_name: &Atom, unresolved_mark: Mark) -> bool {
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = strip_parens(expr)
    else {
        return false;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = left else {
        return false;
    };
    unresolved_exports_member(member, unresolved_mark).as_ref() == Some(public_name)
        && matches!(strip_parens(right), Expr::Object(object) if object.props.is_empty())
}

fn unresolved_exports_member(member: &MemberExpr, unresolved_mark: Mark) -> Option<Atom> {
    let Expr::Ident(object) = strip_parens(&member.obj) else {
        return None;
    };
    if object.sym != *"exports" || object.ctxt.outer() != unresolved_mark {
        return None;
    }
    match &member.prop {
        MemberProp::Ident(property) if is_valid_identifier(property.sym.as_ref()) => {
            Some(property.sym.clone())
        }
        MemberProp::Computed(property) => {
            let Expr::Lit(Lit::Str(value)) = strip_parens(&property.expr) else {
                return None;
            };
            let value = value.value.as_str()?;
            is_valid_identifier(value).then(|| Atom::from(value))
        }
        _ => None,
    }
}

fn enum_member_is_literal_only(member: &EnumMember) -> bool {
    literal_enum_value(&member.value)
        && member
            .reverse
            .as_ref()
            .is_none_or(|(key, value)| literal_enum_value(key) && literal_enum_value(value))
}

fn literal_enum_value(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Lit(_) => true,
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Plus | UnaryOp::Minus,
            arg,
            ..
        }) => matches!(strip_parens(arg), Expr::Lit(Lit::Num(_))),
        _ => false,
    }
}

/// Parse a standalone enum IIFE (no preceding bare var).
/// Returns `(enum_ident, members)` if matched.
fn parse_enum_iife_standalone(stmt: &Stmt) -> Option<(Ident, Vec<EnumMember>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    // Unwrap unary `!` (terser form: `!function(o){...}(o||(o={}))`)
    let expr = strip_unary_bang(expr);
    let Expr::Call(call) = expr else {
        return None;
    };
    let enum_ident = extract_enum_name_from_arg(&call.args)?;

    // Validate that there is no preceding bare var (this is for standalone)
    parse_enum_iife_expr_inner(call, &enum_ident).map(|members| (enum_ident, members))
}

fn parse_enum_iife_expr(expr: &Expr, expected_ident: Option<&Ident>) -> Option<Vec<EnumMember>> {
    let expr = strip_unary_bang(expr);
    let Expr::Call(call) = expr else {
        return None;
    };

    let enum_ident = if let Some(ident) = expected_ident {
        ident.clone()
    } else {
        extract_enum_name_from_arg(&call.args)?
    };

    if let Some(ident) = expected_ident {
        if !validate_enum_iife_arg(&call.args, ident) {
            return None;
        }
    }

    parse_enum_iife_expr_inner(call, &enum_ident)
}

fn parse_enum_iife_expr_inner(call: &CallExpr, enum_ident: &Ident) -> Option<Vec<EnumMember>> {
    // Callee must be a function or block-bodied arrow expression (possibly paren-wrapped)
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    // Single arg matching `Name || (Name = {})`
    if call.args.len() != 1 {
        return None;
    }
    if !validate_enum_iife_arg(&call.args, enum_ident) {
        return None;
    }

    if let Some((inner_param_name, body_expr)) = extract_enum_iife_expr_body(callee) {
        return parse_enum_expr_body(body_expr, inner_param_name);
    }

    // Parse body
    let (inner_param_name, body_stmts) = extract_enum_iife_body(callee)?;
    parse_enum_body(body_stmts, inner_param_name)
}

fn extract_enum_iife_expr_body(expr: &Expr) -> Option<(&Ident, &Expr)> {
    match expr {
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 1 {
                return None;
            }
            let Pat::Ident(param_ident) = &arrow.params[0] else {
                return None;
            };
            let BlockStmtOrExpr::Expr(body) = arrow.body.as_ref() else {
                return None;
            };
            Some((&param_ident.id, body.as_ref()))
        }
        Expr::Paren(paren) => extract_enum_iife_expr_body(&paren.expr),
        _ => None,
    }
}

fn extract_enum_iife_body(expr: &Expr) -> Option<(&Ident, &[Stmt])> {
    match expr {
        Expr::Fn(fn_expr) => extract_fn_expr_body(fn_expr),
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 1 {
                return None;
            }
            let Pat::Ident(param_ident) = &arrow.params[0] else {
                return None;
            };
            let BlockStmtOrExpr::BlockStmt(body) = arrow.body.as_ref() else {
                return None;
            };
            Some((&param_ident.id, &body.stmts))
        }
        Expr::Paren(paren) => extract_enum_iife_body(&paren.expr),
        _ => None,
    }
}

fn extract_fn_expr_body(fn_expr: &FnExpr) -> Option<(&Ident, &[Stmt])> {
    if fn_expr.function.params.len() != 1 {
        return None;
    }
    let Pat::Ident(param_ident) = &fn_expr.function.params[0].pat else {
        return None;
    };
    let body = fn_expr.function.body.as_ref()?;
    Some((&param_ident.id, &body.stmts))
}

fn strip_unary_bang(expr: &Expr) -> &Expr {
    if let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = expr
    {
        return arg.as_ref();
    }
    expr
}

fn extract_enum_name_from_arg(args: &[swc_core::ecma::ast::ExprOrSpread]) -> Option<Ident> {
    if args.len() != 1 {
        return None;
    }
    let expr = strip_parens(&args[0].expr);
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left,
        ..
    }) = expr
    else {
        return None;
    };
    let Expr::Ident(id) = left.as_ref() else {
        return None;
    };
    Some(id.clone())
}

fn validate_enum_iife_arg(args: &[swc_core::ecma::ast::ExprOrSpread], ident: &Ident) -> bool {
    if args.len() != 1 {
        return false;
    }
    is_enum_iife_arg(&args[0].expr, ident)
}

/// Check that expr is `Name || (Name = {})`, `Name || {}`, or an initialized `{}`.
fn is_enum_iife_arg(expr: &Expr, ident: &Ident) -> bool {
    let expr = strip_parens(expr);
    match expr {
        // Standard: Name || (Name = {})
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) => {
            if !matches!(left.as_ref(), Expr::Ident(i) if same_binding(i, ident)) {
                return false;
            }
            let right = strip_parens(right);
            is_assign_empty_obj(right, ident)
                || matches!(right, Expr::Object(o) if o.props.is_empty())
        }
        Expr::Object(o) => o.props.is_empty(),
        _ => false,
    }
}

fn is_assign_empty_obj(expr: &Expr, ident: &Ident) -> bool {
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = expr
    else {
        return false;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Ident(id)) = left else {
        return false;
    };
    if !same_binding(&id.id, ident) {
        return false;
    }
    matches!(right.as_ref(), Expr::Object(o) if o.props.is_empty())
}

// ============================================================
// Body parsing
// ============================================================

/// Parse enum body statements. Returns None if any statement is unrecognized.
fn parse_enum_body(stmts: &[Stmt], enum_param: &Ident) -> Option<Vec<EnumMember>> {
    let mut members = Vec::new();

    for stmt in stmts {
        match stmt {
            // `return EnumName;` - ignore. Minified arrow IIFEs can become
            // `return Enum[Enum.A = 1] = "A", Enum;` after UnConditionals.
            Stmt::Return(return_stmt) => {
                let Some(expr) = &return_stmt.arg else {
                    continue;
                };
                if matches!(strip_parens(expr), Expr::Ident(id) if same_binding(id, enum_param)) {
                    continue;
                }
                members.extend(parse_enum_expr_body(expr, enum_param)?);
            }
            Stmt::Expr(ExprStmt { expr, .. }) => {
                let member = parse_enum_member_expr(expr, enum_param)?;
                members.push(member);
            }
            _ => return None,
        }
    }

    Some(members)
}

fn parse_enum_expr_body(expr: &Expr, enum_param: &Ident) -> Option<Vec<EnumMember>> {
    let mut members = Vec::new();

    match strip_parens(expr) {
        Expr::Seq(seq) => {
            for expr in &seq.exprs {
                let expr = strip_parens(expr);
                if matches!(expr, Expr::Ident(id) if same_binding(id, enum_param)) {
                    continue;
                }
                members.push(parse_enum_member_expr(expr, enum_param)?);
            }
        }
        expr => {
            members.push(parse_enum_member_expr(expr, enum_param)?);
        }
    }

    if members.is_empty() {
        None
    } else {
        Some(members)
    }
}

/// Parse a single enum member expression.
/// Returns None if unrecognized.
fn parse_enum_member_expr(expr: &Expr, enum_param: &Ident) -> Option<EnumMember> {
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = expr
    else {
        return None;
    };

    match left {
        // Numeric member: `Enum[Enum["Key"] = numVal] = "Key"`
        AssignTarget::Simple(SimpleAssignTarget::Member(outer_member)) => {
            if !is_enum_ident(&outer_member.obj, enum_param) {
                return None;
            }
            match &outer_member.prop {
                MemberProp::Computed(outer_computed) => {
                    // Check if inner is `Enum["Key"] = numVal`
                    let inner_expr = strip_parens(&outer_computed.expr);
                    if let Some((key, num_val)) =
                        parse_numeric_forward_assign(inner_expr, enum_param)
                    {
                        // right should be "Key"
                        let reverse_key_str = extract_string_value(right)?;
                        // Build reverse mapping
                        let reverse = Some((
                            num_val.clone(),
                            Box::new(Expr::Lit(Lit::Str(Str {
                                span: DUMMY_SP,
                                value: reverse_key_str.clone(),
                                raw: None,
                            }))),
                        ));
                        return Some(EnumMember {
                            key,
                            value: num_val,
                            reverse,
                        });
                    }
                    None
                }
                _ => None,
            }
        }
        _ => None,
    }
    .or_else(|| {
        // String member: `Enum["Key"] = "VALUE"` or `Enum.Key = "VALUE"`
        let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = left else {
            return None;
        };
        if !is_enum_ident(&member.obj, enum_param) {
            return None;
        }
        let key = extract_member_key(&member.prop)?;
        Some(EnumMember {
            key,
            value: right.clone(),
            reverse: None,
        })
    })
}

/// Parse `Enum["Key"] = numVal` (forward assignment in numeric member pattern)
/// Returns `(EnumKey, Box<Expr> for num_val)` if matched.
fn parse_numeric_forward_assign(expr: &Expr, enum_param: &Ident) -> Option<(EnumKey, Box<Expr>)> {
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = expr
    else {
        return None;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = left else {
        return None;
    };
    if !is_enum_ident(&member.obj, enum_param) {
        return None;
    }
    let key = extract_member_key(&member.prop)?;
    Some((key, right.clone()))
}

fn is_enum_ident(expr: &Expr, enum_param: &Ident) -> bool {
    matches!(expr, Expr::Ident(id) if same_binding(id, enum_param))
}

fn same_binding(left: &Ident, right: &Ident) -> bool {
    left.sym == right.sym && left.ctxt == right.ctxt
}

fn extract_member_key(prop: &MemberProp) -> Option<EnumKey> {
    match prop {
        MemberProp::Ident(ident_name) => Some(EnumKey::Ident(ident_name.sym.clone())),
        MemberProp::Computed(computed) => {
            let inner = strip_parens(&computed.expr);
            if let Expr::Lit(Lit::Str(s)) = inner {
                // Check if it's a valid identifier by converting to &str
                let valid = s.value.as_str().map(is_valid_identifier).unwrap_or(false);
                if valid {
                    // Convert Wtf8Atom -> Atom via the Atom::from impl
                    let atom: Atom = s.value.as_str().unwrap().into();
                    Some(EnumKey::Ident(atom))
                } else {
                    Some(EnumKey::Str(s.value.clone()))
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

fn extract_string_value(expr: &Expr) -> Option<Wtf8Atom> {
    if let Expr::Lit(Lit::Str(s)) = expr {
        Some(s.value.clone())
    } else {
        None
    }
}

fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_alphabetic() && first != '_' && first != '$' {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

fn collect_exported_names(items: &[ModuleItem]) -> HashSet<Atom> {
    let mut names = HashSet::new();
    for item in items {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                collect_decl_names(&export_decl.decl, &mut names);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(named)) => {
                for specifier in &named.specifiers {
                    match specifier {
                        ExportSpecifier::Named(specifier) => {
                            let name = specifier.exported.as_ref().unwrap_or(&specifier.orig);
                            names.insert(name.atom().into_owned());
                        }
                        ExportSpecifier::Namespace(specifier) => {
                            names.insert(specifier.name.atom().into_owned());
                        }
                        ExportSpecifier::Default(specifier) => {
                            names.insert(specifier.exported.sym.clone());
                        }
                    }
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(_))
            | ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(_)) => {
                names.insert("default".into());
            }
            _ => {}
        }
    }
    names
}

/// A minifier can split `var Enum, other = 1` into separate declarations,
/// leaving other statements between the enum binding and its IIFE. Keep the
/// assignment at the IIFE's original position, and accept the split form only
/// when the intervening items mention neither the local binding nor its public
/// `exports` property.
fn has_safe_prior_bare_var(
    items: &[ModuleItem],
    local_ident: &Ident,
    public_name: &Atom,
    unresolved_mark: Mark,
) -> bool {
    let Some(index) = items.iter().rposition(|item| {
        let ModuleItem::Stmt(stmt) = item else {
            return false;
        };
        get_bare_var_decl_ident(stmt)
            .as_ref()
            .is_some_and(|ident| same_binding(ident, local_ident))
    }) else {
        return false;
    };

    items[index + 1..].iter().all(|item| {
        let mut finder = BindingUseFinder {
            binding: local_ident,
            public_name,
            unresolved_mark,
            found: false,
        };
        item.visit_with(&mut finder);
        !finder.found
    })
}

struct BindingUseFinder<'a> {
    binding: &'a Ident,
    public_name: &'a Atom,
    unresolved_mark: Mark,
    found: bool,
}

impl Visit for BindingUseFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        self.found |= same_binding(ident, self.binding);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        self.found |= unresolved_exports_member(member, self.unresolved_mark).as_ref()
            == Some(self.public_name);
        member.visit_children_with(self);
    }
}

// ============================================================
// Building output
// ============================================================

fn build_named_enum_export(local_ident: &Ident, public_name: Atom) -> ModuleItem {
    let exported = (local_ident.sym != public_name)
        .then(|| ModuleExportName::Ident(IdentName::new(public_name, DUMMY_SP).into()));
    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
        span: DUMMY_SP,
        specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
            span: DUMMY_SP,
            orig: ModuleExportName::Ident(local_ident.clone()),
            exported,
            is_type_only: false,
        })],
        src: None,
        type_only: false,
        with: None,
    }))
}

/// Build `var Name = { ... }` using the original var stmt's structure
fn build_enum_var_decl(ident: &Ident, members: Vec<EnumMember>, original_stmt: &Stmt) -> Stmt {
    let obj = build_enum_object(members);

    // Get VarDeclKind from original
    let kind = if let Stmt::Decl(Decl::Var(v)) = original_stmt {
        v.kind
    } else {
        VarDeclKind::Var
    };

    let var_span = if original_stmt.span().lo.0 != 0 {
        original_stmt.span()
    } else {
        DUMMY_SP
    };
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: var_span,
        ctxt: Default::default(),
        kind,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: ident.clone(),
                type_ann: None,
            }),
            init: Some(Box::new(obj)),
            definite: false,
        }],
    })))
}

/// Build an assignment statement for standalone IIFE (no preceding bare var):
/// `Name = { ... }` as an ExprStmt
fn build_enum_assign_stmt(ident: Ident, members: Vec<EnumMember>, original_span: Span) -> Stmt {
    let obj = build_enum_object(members);
    let stmt_span = if original_span.lo.0 != 0 {
        original_span
    } else {
        DUMMY_SP
    };
    Stmt::Expr(ExprStmt {
        span: stmt_span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Ident(
                swc_core::ecma::ast::BindingIdent {
                    id: ident,
                    type_ann: None,
                },
            )),
            right: Box::new(obj),
        })),
    })
}

fn build_enum_object(members: Vec<EnumMember>) -> Expr {
    let mut props: Vec<PropOrSpread> = Vec::new();

    // Forward mappings
    for member in &members {
        let key = make_forward_prop_name(&member.key);
        props.push(PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
            key,
            value: member.value.clone(),
        }))));
    }

    // Reverse mappings (only for numeric values)
    let has_reverse = members.iter().any(|m| m.reverse.is_some());

    if has_reverse {
        for member in &members {
            if let Some((num_key_expr, str_val)) = &member.reverse {
                let key = make_reverse_prop_name(num_key_expr);
                props.push(PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                    key,
                    value: str_val.clone(),
                }))));
            }
        }
    }

    Expr::Object(ObjectLit {
        span: DUMMY_SP,
        props,
    })
}

fn make_forward_prop_name(key: &EnumKey) -> PropName {
    match key {
        EnumKey::Ident(sym) => PropName::Ident(IdentName::new(sym.clone(), DUMMY_SP)),
        EnumKey::Str(sym) => PropName::Str(Str {
            span: DUMMY_SP,
            value: sym.clone(),
            raw: None,
        }),
    }
}

fn make_reverse_prop_name(num_key_expr: &Expr) -> PropName {
    match num_key_expr {
        // Positive numeric literal → use Num prop name
        Expr::Lit(Lit::Num(n)) => PropName::Num(Number {
            span: DUMMY_SP,
            value: n.value,
            raw: None,
        }),
        // Negative number or any other expression → computed
        _ => PropName::Computed(ComputedPropName {
            span: DUMMY_SP,
            expr: Box::new(num_key_expr.clone()),
        }),
    }
}
