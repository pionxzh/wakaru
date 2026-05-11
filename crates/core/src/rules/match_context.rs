use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{Expr, Function, Ident, Lit, MemberProp, Pat};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub sym: Atom,
    pub ctxt: SyntaxContext,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MatchContext {
    slots: Vec<(&'static str, Binding)>,
}

impl MatchContext {
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// Extract function params as named binding slots.
    /// Returns `None` if param count doesn't match or any param is not a simple ident.
    pub fn from_params(func: &Function, names: &[&'static str]) -> Option<Self> {
        if func.params.len() != names.len() {
            return None;
        }
        let mut ctx = Self::new();
        for (param, name) in func.params.iter().zip(names.iter()) {
            let Pat::Ident(bi) = &param.pat else {
                return None;
            };
            ctx.declare(name, bi.id.sym.clone(), bi.id.ctxt);
        }
        Some(ctx)
    }

    pub fn declare(&mut self, name: &'static str, sym: Atom, ctxt: SyntaxContext) {
        if let Some(slot) = self.slots.iter_mut().find(|(n, _)| *n == name) {
            slot.1 = Binding { sym, ctxt };
        } else {
            self.slots.push((name, Binding { sym, ctxt }));
        }
    }

    pub fn get(&self, name: &str) -> Option<&Binding> {
        self.slots.iter().find(|(n, _)| *n == name).map(|(_, b)| b)
    }

    /// Check if `expr` is an identifier matching the named binding slot.
    pub fn is_binding(&self, expr: &Expr, name: &str) -> bool {
        let Some(binding) = self.get(name) else {
            return false;
        };
        matches!(expr, Expr::Ident(id) if id.sym == binding.sym && id.ctxt == binding.ctxt)
    }

    /// Check if an `Ident` node matches the named binding slot.
    /// Use when you already have an extracted `&Ident` rather than an `&Expr`.
    pub fn is_ident(&self, ident: &Ident, name: &str) -> bool {
        let Some(binding) = self.get(name) else {
            return false;
        };
        ident.sym == binding.sym && ident.ctxt == binding.ctxt
    }

    /// Check if `expr` is `<binding>.prop_name`.
    pub fn is_member_of(&self, expr: &Expr, name: &str, prop_name: &str) -> bool {
        let Expr::Member(member) = expr else {
            return false;
        };
        if !self.is_binding(&member.obj, name) {
            return false;
        }
        is_member_prop_name(&member.prop, prop_name)
    }

    /// Raw `(sym, ctxt)` for APIs that still need the tuple form.
    #[allow(dead_code)]
    pub fn binding_key(&self, name: &str) -> Option<(&Atom, SyntaxContext)> {
        self.get(name).map(|b| (&b.sym, b.ctxt))
    }
}

fn is_member_prop_name(prop: &MemberProp, name: &str) -> bool {
    match prop {
        MemberProp::Ident(id) => id.sym.as_ref() == name,
        MemberProp::Computed(c) => {
            matches!(c.expr.as_ref(), Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some(name))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::atoms::Atom;
    use swc_core::common::{SyntaxContext, DUMMY_SP, GLOBALS};
    use swc_core::ecma::ast::{Ident, IdentName, MemberExpr, MemberProp};

    fn make_ident(sym: &str, ctxt: SyntaxContext) -> Box<Expr> {
        Box::new(Expr::Ident(Ident {
            ctxt,
            sym: Atom::from(sym),
            span: DUMMY_SP,
            optional: false,
        }))
    }

    fn make_member(obj: &str, obj_ctxt: SyntaxContext, prop: &str) -> Box<Expr> {
        Box::new(Expr::Member(MemberExpr {
            span: DUMMY_SP,
            obj: make_ident(obj, obj_ctxt),
            prop: MemberProp::Ident(IdentName {
                span: DUMMY_SP,
                sym: Atom::from(prop),
            }),
        }))
    }

    fn make_ctx() -> MatchContext {
        let mut ctx = MatchContext::new();
        ctx.declare("a", Atom::from("x"), SyntaxContext::empty());
        ctx.declare(
            "b",
            Atom::from("y"),
            SyntaxContext::empty().apply_mark(swc_core::common::Mark::new()),
        );
        ctx
    }

    #[test]
    fn is_binding_matches_same_ident() {
        GLOBALS.set(&Default::default(), || {
            let ctx = make_ctx();
            let expr = make_ident("x", SyntaxContext::empty());
            assert!(ctx.is_binding(&expr, "a"));
        });
    }

    #[test]
    fn is_binding_rejects_wrong_name() {
        GLOBALS.set(&Default::default(), || {
            let ctx = make_ctx();
            let expr = make_ident("z", SyntaxContext::empty());
            assert!(!ctx.is_binding(&expr, "a"));
        });
    }

    #[test]
    fn is_binding_rejects_wrong_ctxt() {
        GLOBALS.set(&Default::default(), || {
            let ctx = make_ctx();
            let wrong_ctxt = SyntaxContext::empty().apply_mark(swc_core::common::Mark::new());
            let expr = make_ident("x", wrong_ctxt);
            assert!(!ctx.is_binding(&expr, "a"));
        });
    }

    #[test]
    fn is_binding_rejects_unknown_slot() {
        GLOBALS.set(&Default::default(), || {
            let ctx = make_ctx();
            let expr = make_ident("x", SyntaxContext::empty());
            assert!(!ctx.is_binding(&expr, "nonexistent"));
        });
    }

    #[test]
    fn is_member_of_matches() {
        GLOBALS.set(&Default::default(), || {
            let ctx = make_ctx();
            let expr = make_member("x", SyntaxContext::empty(), "__esModule");
            assert!(ctx.is_member_of(&expr, "a", "__esModule"));
        });
    }

    #[test]
    fn is_member_of_rejects_wrong_prop() {
        GLOBALS.set(&Default::default(), || {
            let ctx = make_ctx();
            let expr = make_member("x", SyntaxContext::empty(), "other");
            assert!(!ctx.is_member_of(&expr, "a", "__esModule"));
        });
    }

    #[test]
    fn is_member_of_rejects_wrong_object() {
        GLOBALS.set(&Default::default(), || {
            let ctx = make_ctx();
            let expr = make_member("z", SyntaxContext::empty(), "__esModule");
            assert!(!ctx.is_member_of(&expr, "a", "__esModule"));
        });
    }

    #[test]
    fn declare_overwrites_existing_slot() {
        let mut ctx = MatchContext::new();
        ctx.declare("a", Atom::from("x"), SyntaxContext::empty());
        ctx.declare("a", Atom::from("y"), SyntaxContext::empty());
        assert_eq!(ctx.get("a").unwrap().sym, Atom::from("y"));
        assert_eq!(ctx.slots.len(), 1);
    }

    #[test]
    fn from_params_rejects_wrong_count() {
        GLOBALS.set(&Default::default(), || {
            use swc_core::ecma::ast::{BlockStmt, Param};
            let func = Function {
                params: vec![Param {
                    span: DUMMY_SP,
                    decorators: vec![],
                    pat: Pat::Ident(
                        Ident {
                            ctxt: SyntaxContext::empty(),
                            sym: Atom::from("a"),
                            span: DUMMY_SP,
                            optional: false,
                        }
                        .into(),
                    ),
                }],
                decorators: vec![],
                span: DUMMY_SP,
                ctxt: SyntaxContext::empty(),
                body: Some(BlockStmt {
                    span: DUMMY_SP,
                    ctxt: SyntaxContext::empty(),
                    stmts: vec![],
                }),
                is_generator: false,
                is_async: false,
                type_params: None,
                return_type: None,
            };
            assert!(MatchContext::from_params(&func, &["a", "b"]).is_none());
        });
    }
}
