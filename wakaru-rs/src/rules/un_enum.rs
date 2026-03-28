use swc_core::atoms::{Atom, Wtf8Atom};
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BindingIdent, CallExpr, Callee,
    ComputedPropName, Decl, Expr, ExprStmt, FnExpr, Ident, IdentName, KeyValueProp, Lit,
    MemberProp, ModuleItem, Number, ObjectLit, Pat, Prop, PropName, PropOrSpread,
    SimpleAssignTarget, Stmt, Str, UnaryExpr, UnaryOp, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnEnum;

impl VisitMut for UnEnum {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        process_module_items_for_enum(items);
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

fn process_module_items_for_enum(items: &mut Vec<ModuleItem>) {
    let old: Vec<ModuleItem> = std::mem::take(items);
    let mut iter = old.into_iter().peekable();

    while let Some(item) = iter.next() {
        match item {
            ModuleItem::Stmt(stmt) => {
                // Check if this is a bare var decl like `var Direction;`
                if let Some(bare_var_name) = get_bare_var_decl_name(&stmt) {
                    if let Some(peeked) = iter.peek() {
                        if let ModuleItem::Stmt(next_stmt) = peeked {
                            if let Some(members) = parse_enum_iife(next_stmt, &bare_var_name) {
                                // Consume the IIFE statement
                                iter.next();
                                let new_stmt = build_enum_var_decl(&bare_var_name, members, &stmt);
                                items.push(ModuleItem::Stmt(new_stmt));
                                continue;
                            }
                        }
                    }
                }

                // Try standalone enum IIFE (without preceding bare var)
                if let Some((name, members)) = parse_enum_iife_standalone(&stmt) {
                    let new_stmt = build_enum_assign_stmt(name, members);
                    items.push(ModuleItem::Stmt(new_stmt));
                    continue;
                }

                items.push(ModuleItem::Stmt(stmt));
            }
            other => items.push(other),
        }
    }
}

fn process_stmts_for_enum(stmts: &mut Vec<Stmt>) {
    let old: Vec<Stmt> = std::mem::take(stmts);
    let mut iter = old.into_iter().peekable();

    while let Some(stmt) = iter.next() {
        if let Some(bare_var_name) = get_bare_var_decl_name(&stmt) {
            if let Some(peeked) = iter.peek() {
                if let Some(members) = parse_enum_iife(peeked, &bare_var_name) {
                    iter.next(); // consume the IIFE
                    let new_stmt = build_enum_var_decl(&bare_var_name, members, &stmt);
                    stmts.push(new_stmt);
                    continue;
                }
            }
        }

        if let Some((name, members)) = parse_enum_iife_standalone(&stmt) {
            let new_stmt = build_enum_assign_stmt(name, members);
            stmts.push(new_stmt);
            continue;
        }

        stmts.push(stmt);
    }
}

// ============================================================
// Detection helpers
// ============================================================

/// Check if stmt is `var Name;` (VarDecl with 1 declarator, no init)
fn get_bare_var_decl_name(stmt: &Stmt) -> Option<Atom> {
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
    Some(id.sym.clone())
}

/// Parse an enum IIFE where the inner function param name matches `expected_name`.
/// Also handles mangled enums where the param name differs from `expected_name`
/// (the arg `expected_name || (expected_name = {})` determines the enum name).
fn parse_enum_iife(stmt: &Stmt, expected_name: &Atom) -> Option<Vec<EnumMember>> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    parse_enum_iife_expr(expr, Some(expected_name))
}

/// Parse a standalone enum IIFE (no preceding bare var).
/// Returns `(enum_name, members)` if matched.
fn parse_enum_iife_standalone(stmt: &Stmt) -> Option<(Atom, Vec<EnumMember>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    // Unwrap unary `!` (terser form: `!function(o){...}(o||(o={}))`)
    let expr = strip_unary_bang(expr);
    let Expr::Call(call) = expr else {
        return None;
    };
    let enum_name = extract_enum_name_from_arg(&call.args)?;

    // Validate that there is no preceding bare var (this is for standalone)
    parse_enum_iife_expr_inner(call, &enum_name, None).map(|members| (enum_name, members))
}

fn parse_enum_iife_expr(expr: &Expr, expected_name: Option<&Atom>) -> Option<Vec<EnumMember>> {
    let expr = strip_unary_bang(expr);
    let Expr::Call(call) = expr else {
        return None;
    };

    let enum_name = if let Some(n) = expected_name {
        n.clone()
    } else {
        extract_enum_name_from_arg(&call.args)?
    };

    if let Some(n) = expected_name {
        if !validate_enum_iife_arg(&call.args, n) {
            return None;
        }
    }

    parse_enum_iife_expr_inner(call, &enum_name, expected_name)
}

fn parse_enum_iife_expr_inner(
    call: &CallExpr,
    enum_name: &Atom,
    _expected_name: Option<&Atom>,
) -> Option<Vec<EnumMember>> {
    // Callee must be FnExpr (possibly paren-wrapped)
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let fn_expr = extract_fn_expr(callee)?;

    // Single param
    if fn_expr.function.params.len() != 1 {
        return None;
    }
    let Pat::Ident(param_ident) = &fn_expr.function.params[0].pat else {
        return None;
    };
    let inner_param_name = &param_ident.id.sym;

    // Single arg matching `Name || (Name = {})`
    if call.args.len() != 1 {
        return None;
    }
    if !validate_enum_iife_arg(&call.args, enum_name) {
        return None;
    }

    // Parse body
    let body = fn_expr.function.body.as_ref()?;
    parse_enum_body(&body.stmts, inner_param_name)
}

fn extract_fn_expr(expr: &Expr) -> Option<&FnExpr> {
    match expr {
        Expr::Fn(fn_expr) => Some(fn_expr),
        Expr::Paren(paren) => extract_fn_expr(&paren.expr),
        _ => None,
    }
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

fn extract_enum_name_from_arg(args: &[swc_core::ecma::ast::ExprOrSpread]) -> Option<Atom> {
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
    Some(id.sym.clone())
}

fn validate_enum_iife_arg(args: &[swc_core::ecma::ast::ExprOrSpread], name: &Atom) -> bool {
    if args.len() != 1 {
        return false;
    }
    is_enum_iife_arg(&args[0].expr, name)
}

/// Check that expr is `Name || (Name = {})` or `Name || {}`
fn is_enum_iife_arg(expr: &Expr, name: &Atom) -> bool {
    let expr = strip_parens(expr);
    match expr {
        // Standard: Name || (Name = {})
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) => {
            if !matches!(left.as_ref(), Expr::Ident(i) if &i.sym == name) {
                return false;
            }
            let right = strip_parens(right);
            is_assign_empty_obj(right, name)
                || matches!(right, Expr::Object(o) if o.props.is_empty())
        }
        _ => false,
    }
}

fn is_assign_empty_obj(expr: &Expr, name: &Atom) -> bool {
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
    if &id.id.sym != name {
        return false;
    }
    matches!(right.as_ref(), Expr::Object(o) if o.props.is_empty())
}

fn strip_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_parens(&paren.expr),
        _ => expr,
    }
}

// ============================================================
// Body parsing
// ============================================================

/// Parse enum body statements. Returns None if any statement is unrecognized.
fn parse_enum_body(stmts: &[Stmt], enum_param: &Atom) -> Option<Vec<EnumMember>> {
    let mut members = Vec::new();

    for stmt in stmts {
        match stmt {
            // `return EnumName;` - ignore
            Stmt::Return(_) => continue,
            Stmt::Expr(ExprStmt { expr, .. }) => {
                let member = parse_enum_member_expr(expr, enum_param)?;
                members.push(member);
            }
            _ => return None,
        }
    }

    Some(members)
}

/// Parse a single enum member expression.
/// Returns None if unrecognized.
fn parse_enum_member_expr(expr: &Expr, enum_param: &Atom) -> Option<EnumMember> {
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
fn parse_numeric_forward_assign(expr: &Expr, enum_param: &Atom) -> Option<(EnumKey, Box<Expr>)> {
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

fn is_enum_ident(expr: &Expr, enum_param: &Atom) -> bool {
    matches!(expr, Expr::Ident(id) if &id.sym == enum_param)
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

// ============================================================
// Building output
// ============================================================

/// Build `var Name = { ... }` using the original var stmt's structure
fn build_enum_var_decl(name: &Atom, members: Vec<EnumMember>, original_stmt: &Stmt) -> Stmt {
    let obj = build_enum_object(members);

    // Get VarDeclKind from original
    let kind = if let Stmt::Decl(Decl::Var(v)) = original_stmt {
        v.kind
    } else {
        VarDeclKind::Var
    };

    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: Ident::new_no_ctxt(name.clone(), DUMMY_SP),
                type_ann: None,
            }),
            init: Some(Box::new(obj)),
            definite: false,
        }],
    })))
}

/// Build an assignment statement for standalone IIFE (no preceding bare var):
/// `Name = { ... }` as an ExprStmt
fn build_enum_assign_stmt(name: Atom, members: Vec<EnumMember>) -> Stmt {
    let obj = build_enum_object(members);
    Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Ident(
                swc_core::ecma::ast::BindingIdent {
                    id: Ident::new_no_ctxt(name, DUMMY_SP),
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
