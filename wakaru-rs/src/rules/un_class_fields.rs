use std::collections::HashMap;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    CallExpr, Callee, Class, ClassMember, Expr, ExprStmt, MemberExpr, MemberProp, Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Inline `__init*()` method bodies into the constructor.
///
/// Babel/SWC class field transpilation produces:
/// ```js
/// class Foo {
///     __init() { this._x = 1; }
///     __init2() { this._y = 2; }
///     constructor() {
///         Foo.prototype.__init.call(this);
///         Foo.prototype.__init2.call(this);
///     }
/// }
/// ```
/// This rule inlines them back:
/// ```js
/// class Foo {
///     constructor() {
///         this._x = 1;
///         this._y = 2;
///     }
/// }
/// ```
pub struct UnClassFields;

impl VisitMut for UnClassFields {
    fn visit_mut_class(&mut self, class: &mut Class) {
        class.visit_mut_children_with(self);

        // Collect __init* method bodies
        let mut init_bodies: HashMap<Atom, Vec<Stmt>> = HashMap::new();
        for member in &class.body {
            let ClassMember::Method(method) = member else {
                continue;
            };
            let Some(name) = prop_name_str(&method.key) else {
                continue;
            };
            if !name.starts_with("__init") {
                continue;
            }
            if method.is_static {
                continue;
            }
            let Some(body) = &method.function.body else {
                continue;
            };
            // All statements must be `this.X = expr` assignments
            if body.stmts.is_empty() {
                continue;
            }
            let all_this_assigns = body.stmts.iter().all(|s| is_this_assignment(s));
            if !all_this_assigns {
                continue;
            }
            init_bodies.insert(Atom::from(name), body.stmts.clone());
        }

        if init_bodies.is_empty() {
            return;
        }

        // Find constructor and inline the __init calls
        let class_name = self.find_class_name(class);
        let mut inlined_names: std::collections::HashSet<Atom> = std::collections::HashSet::new();

        for member in &mut class.body {
            let ClassMember::Constructor(ctor) = member else {
                continue;
            };
            let Some(body) = &mut ctor.body else {
                continue;
            };

            let mut new_stmts = Vec::with_capacity(body.stmts.len());
            for stmt in body.stmts.drain(..) {
                if let Some(init_name) = extract_prototype_init_call(&stmt, &class_name) {
                    if let Some(init_stmts) = init_bodies.get(&init_name) {
                        new_stmts.extend(init_stmts.iter().cloned());
                        inlined_names.insert(init_name);
                        continue;
                    }
                }
                new_stmts.push(stmt);
            }
            body.stmts = new_stmts;
        }

        if inlined_names.is_empty() {
            return;
        }

        // Remove only the __init* methods that were actually inlined
        class.body.retain(|member| {
            let ClassMember::Method(method) = member else {
                return true;
            };
            if method.is_static {
                return true;
            }
            let Some(name) = prop_name_str(&method.key) else {
                return true;
            };
            if inlined_names.contains(&Atom::from(name.as_str())) {
                return false; // remove
            }
            true
        });
    }
}

impl UnClassFields {
    fn find_class_name(&self, _class: &Class) -> Option<Atom> {
        // Classes in class declarations have their name set by the parent node,
        // not on the Class itself. We'll match by checking the prototype call pattern.
        None // Will match any class name in extract_prototype_init_call
    }
}

fn prop_name_str(key: &swc_core::ecma::ast::PropName) -> Option<String> {
    match key {
        swc_core::ecma::ast::PropName::Ident(id) => Some(id.sym.to_string()),
        swc_core::ecma::ast::PropName::Str(s) => s.value.as_str().map(|s| s.to_string()),
        _ => None,
    }
}

/// Check if statement is `this.X = expr`
fn is_this_assignment(stmt: &Stmt) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = &**expr else {
        return false;
    };
    let swc_core::ecma::ast::AssignTarget::Simple(swc_core::ecma::ast::SimpleAssignTarget::Member(
        member,
    )) = &assign.left
    else {
        return false;
    };
    matches!(&*member.obj, Expr::This(_))
}

/// Extract `__initN` name from `ClassName.prototype.__initN.call(this)`
fn extract_prototype_init_call(stmt: &Stmt, _class_name: &Option<Atom>) -> Option<Atom> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee),
        args,
        ..
    }) = &**expr
    else {
        return None;
    };

    // callee: X.prototype.__initN.call
    let Expr::Member(MemberExpr {
        obj: call_obj,
        prop: MemberProp::Ident(call_prop),
        ..
    }) = &**callee
    else {
        return None;
    };
    if call_prop.sym.as_ref() != "call" {
        return None;
    }

    // call_obj: X.prototype.__initN
    let Expr::Member(MemberExpr {
        obj: proto_obj,
        prop: MemberProp::Ident(init_prop),
        ..
    }) = &**call_obj
    else {
        return None;
    };
    let init_name = &init_prop.sym;
    if !init_name.starts_with("__init") {
        return None;
    }

    // proto_obj: X.prototype
    let Expr::Member(MemberExpr {
        prop: MemberProp::Ident(proto_prop),
        ..
    }) = &**proto_obj
    else {
        return None;
    };
    if proto_prop.sym.as_ref() != "prototype" {
        return None;
    }

    // args must be exactly [this]
    if args.len() != 1 {
        return None;
    }
    if !matches!(&*args[0].expr, Expr::This(_)) {
        return None;
    }

    Some(init_name.clone())
}
