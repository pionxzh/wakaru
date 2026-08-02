//! Restore object literals from Babel's loose-mode computed-properties output.
//!
//! `@babel/plugin-transform-computed-properties` with `loose: true` (or the
//! Babel 7 `setComputedProperties: true` assumption, or Babel 6's
//! `babel-plugin-transform-es2015-computed-properties` with `loose: true`)
//! lowers an object literal containing a computed key into a temporary plus a
//! sequence of member assignments:
//!
//! ```js
//! var n = { a: 1, [k]: 2, b: 3 };
//! // ->
//! var _n;
//! var n = (_n = { a: 1 }, _n[k] = 2, _n.b = 3, _n);
//! ```
//!
//! Terser's `compress` pass — on by default for production bundles — folds the
//! seed into the first assignment, so the shape actually found in the wild is
//! usually:
//!
//! ```js
//! var _n;
//! var n = ((_n = { a: 1 })[k] = 2, _n.b = 3, _n);
//! ```
//!
//! Both forms are matched.
//!
//! Properties before the first computed key stay in the seed object literal;
//! the first computed key and everything after it become assignments, whether
//! or not those later keys were computed in the source. So the exact original
//! spelling is not recoverable — only the fact that the literal had at least
//! one computed key. This rule folds the sequence back into a single object
//! literal, choosing the most readable key form for each assignment.
//!
//! The spec-mode counterpart of the same transform emits `_defineProperty`
//! calls and is handled by [`super::un_define_property`].
//!
//! Safety conditions, all required:
//!
//! * The temporary has a real, never-initialized `var` declaration (`var _n;`).
//!   Lexical declarations are excluded because a later `let _n;` is still in
//!   its temporal dead zone at the pattern, and TypeScript ambient declarations
//!   do not create runtime bindings. An undeclared global would make erasing
//!   `_n = {}` drop a global write, and an unresolved binding would drop a
//!   `ReferenceError`.
//! * Every occurrence of the temporary in the module is accounted for by the
//!   matched pattern and the binding is not exported, so folding it away cannot
//!   be observed. Keys and values that mention the temporary are rejected by
//!   the same count.
//! * The seed object literal has no accessor property and no `__proto__` key:
//!   assigning to a key that an accessor covers calls that accessor, and a
//!   `__proto__` key installs a prototype whose setters later assignments would
//!   hit. Neither is equivalent to defining the property in one literal.
//! * No assignment uses a statically-known `__proto__` key, because
//!   `obj.__proto__ = x` invokes the inherited setter while `__proto__` in a
//!   computed key position defines an own property.
//!
//! Assumption: `set_computed_properties` (see `docs/rewrite-assumptions.md`).
//! Assignment and definition still differ for a dynamic key that turns out to
//! be `__proto__` at runtime, inherited descriptors that intercept or block a
//! write, and inferred names on anonymous function/class values. Babel exposes
//! this transform as a loose assumption for exactly that reason, so the rule is
//! gated to `standard` and above.

use std::collections::{HashMap, HashSet};

use swc_core::common::Spanned;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, ComputedPropName, ExportSpecifier, Expr, KeyValueProp, Lit, MemberProp,
    Module, ModuleDecl, ModuleExportName, ModuleItem, ObjectLit, Pat, Prop, PropName, PropOrSpread,
    SeqExpr, SimpleAssignTarget, VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::binding_facts::collect_binding_facts;
use super::dead_decls::{extend_consumed_uninitialized_expr, remove_consumed_uninitialized_decls};
use super::decl_utils::{binding_id, collect_decl_binding_ids, ident_matches_binding, BindingId};
use super::helper_matcher::count_binding_refs;
use super::RewriteLevel;

use crate::utils::paren::strip_parens;

const PROTO: &str = "__proto__";

pub struct UnComputedProperties {
    rewrite_level: RewriteLevel,
    foldable_temp_bindings: HashSet<BindingId>,
    binding_references: HashMap<BindingId, usize>,
    consumed_uninitialized_bindings: HashSet<BindingId>,
}

impl UnComputedProperties {
    pub fn new(rewrite_level: RewriteLevel) -> Self {
        Self {
            rewrite_level,
            foldable_temp_bindings: HashSet::new(),
            binding_references: HashMap::new(),
            consumed_uninitialized_bindings: HashSet::new(),
        }
    }
}

impl VisitMut for UnComputedProperties {
    fn visit_mut_module(&mut self, module: &mut Module) {
        if self.rewrite_level < RewriteLevel::Standard {
            return;
        }

        let facts = collect_binding_facts(module);
        self.foldable_temp_bindings = collect_foldable_temp_bindings(module);
        self.binding_references = facts.references;
        self.consumed_uninitialized_bindings.clear();

        module.visit_mut_children_with(self);

        remove_consumed_uninitialized_decls(module, &self.consumed_uninitialized_bindings);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        // Children first: a nested lowering puts one sequence inside an outer
        // assignment's value (`_n[k] = (_k = {}, _k[j] = 1, _k)`), and the outer
        // match requires that value to be free of stray temp references — which
        // only holds once the inner sequence has collapsed.
        expr.visit_mut_children_with(self);

        let Some(folded) = self.try_fold_sequence(expr) else {
            return;
        };

        extend_consumed_uninitialized_expr(
            &mut self.consumed_uninitialized_bindings,
            expr,
            &folded,
            &self.foldable_temp_bindings,
            &self.binding_references,
        );
        *expr = folded;
    }
}

impl UnComputedProperties {
    fn try_fold_sequence(&self, expr: &Expr) -> Option<Expr> {
        let Expr::Seq(seq) = strip_parens(expr) else {
            return None;
        };
        // Shortest producible shape: `(_n = {})[k] = v, _n`.
        if seq.exprs.len() < 2 {
            return None;
        }

        let Expr::Ident(temp) = strip_parens(seq.exprs.last()?) else {
            return None;
        };
        let temp_binding = binding_id(temp);

        // Only a declared-but-uninitialized binding can be folded away; see the
        // module docs for why globals and unresolved names are excluded.
        if !self.foldable_temp_bindings.contains(&temp_binding) {
            return None;
        }

        let seed_index = find_seed_index(seq, &temp_binding)?;
        let assignments = &seq.exprs[seed_index + 1..seq.exprs.len() - 1];

        // The temp must not be read before the pattern seeds it, or the prefix
        // would observe the previous value.
        if count_expr_binding_refs(&seq.exprs[..seed_index], &temp_binding) != 0 {
            return None;
        }

        // Structural occurrences: the seed assignment target, one member object
        // per following assignment, and the trailing read. Any surplus means the
        // temp is mentioned inside a key or a value, which folding would break.
        let structural_refs = assignments.len() + 2;
        if count_expr_binding_refs(&seq.exprs[seed_index..], &temp_binding) != structural_refs {
            return None;
        }
        // `binding_references` counts the declarator identifier too, so a
        // module-wide total of exactly one more than the pattern proves the temp
        // is not observed anywhere else.
        if self.binding_references.get(&temp_binding).copied() != Some(structural_refs + 1) {
            return None;
        }

        let seed = match_seed(&seq.exprs[seed_index], &temp_binding)?;
        if !seed_object_is_foldable(seed.object) {
            return None;
        }
        // Babel always moves at least one property out of the literal, so a seed
        // with nothing assigned onto it is some other shape.
        if seed.chained.is_none() && assignments.is_empty() {
            return None;
        }

        let mut props = seed.object.props.clone();
        if let Some((prop, value)) = seed.chained {
            props.push(key_value_prop(prop, value)?);
        }
        for assignment in assignments {
            props.push(assignment_to_prop(assignment, &temp_binding)?);
        }

        let folded = Expr::Object(ObjectLit {
            span: expr.span(),
            props,
        });

        if seed_index == 0 {
            return Some(folded);
        }

        // A minifier can merge unrelated expressions ahead of the pattern; keep
        // that prefix and replace only the object-building suffix.
        let mut exprs = seq.exprs[..seed_index].to_vec();
        exprs.push(Box::new(folded));
        Some(Expr::Seq(SeqExpr {
            span: expr.span(),
            exprs,
        }))
    }
}

fn collect_foldable_temp_bindings(module: &Module) -> HashSet<BindingId> {
    let mut collector = UninitializedVarCollector::default();
    module.visit_with(&mut collector);

    let exported = collect_exported_binding_ids(module);
    collector
        .bindings
        .retain(|binding| !exported.contains(binding));
    collector.bindings
}

#[derive(Default)]
struct UninitializedVarCollector {
    bindings: HashSet<BindingId>,
}

impl Visit for UninitializedVarCollector {
    fn visit_var_decl(&mut self, var: &VarDecl) {
        if var.kind == VarDeclKind::Var && !var.declare {
            for declarator in &var.decls {
                if declarator.init.is_none() {
                    let Pat::Ident(binding) = &declarator.name else {
                        continue;
                    };
                    self.bindings.insert(binding_id(&binding.id));
                }
            }
        }

        var.visit_children_with(self);
    }
}

fn collect_exported_binding_ids(module: &Module) -> HashSet<BindingId> {
    let mut bindings = HashSet::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(module_decl) = item else {
            continue;
        };

        match module_decl {
            ModuleDecl::ExportDecl(export) => {
                collect_decl_binding_ids(&export.decl, &mut bindings);
            }
            ModuleDecl::ExportNamed(export) if export.src.is_none() && !export.type_only => {
                for specifier in &export.specifiers {
                    let ExportSpecifier::Named(named) = specifier else {
                        continue;
                    };
                    if named.is_type_only {
                        continue;
                    }
                    let ModuleExportName::Ident(local) = &named.orig else {
                        continue;
                    };
                    bindings.insert(binding_id(local));
                }
            }
            _ => {}
        }
    }

    bindings
}

/// The element that seeds the object, in one of the two producible forms.
struct Seed<'a> {
    /// The literal the temp is initialized with.
    object: &'a ObjectLit,
    /// Set for the chained form, where the seed element also carries the first
    /// property.
    chained: Option<(&'a MemberProp, &'a Expr)>,
}

/// Index of the element that seeds the object, requiring every element between
/// it and the trailing read to be a member assignment on the same temp.
fn find_seed_index(seq: &SeqExpr, temp: &BindingId) -> Option<usize> {
    let last_index = seq.exprs.len() - 1;
    (0..last_index).find(|&index| {
        match_seed(&seq.exprs[index], temp).is_some()
            && seq.exprs[index + 1..last_index]
                .iter()
                .all(|assignment| member_assignment_prop(assignment, temp).is_some())
    })
}

/// Match `temp = { ... }`, or the shape a minifier folds it into,
/// `(temp = { ... })[key] = value`. Terser's `compress` pass produces the
/// second form whenever the first property assignment directly follows the
/// seed, which is the common case in production bundles.
fn match_seed<'a>(expr: &'a Expr, temp: &BindingId) -> Option<Seed<'a>> {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }

    match &assign.left {
        AssignTarget::Simple(SimpleAssignTarget::Ident(target)) => {
            if !ident_matches_binding(&target.id, temp) {
                return None;
            }
            match strip_parens(&assign.right) {
                Expr::Object(object) => Some(Seed {
                    object,
                    chained: None,
                }),
                _ => None,
            }
        }
        AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
            let object = seed_assignment_object(&member.obj, temp)?;
            Some(Seed {
                object,
                chained: Some((&member.prop, &assign.right)),
            })
        }
        _ => None,
    }
}

/// The object literal of a nested `temp = { ... }` used as a member target.
fn seed_assignment_object<'a>(expr: &'a Expr, temp: &BindingId) -> Option<&'a ObjectLit> {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assign.left else {
        return None;
    };
    if !ident_matches_binding(&target.id, temp) {
        return None;
    }
    match strip_parens(&assign.right) {
        Expr::Object(object) => Some(object),
        _ => None,
    }
}

/// The member property of `temp.k = v` / `temp[k] = v`, or `None` when the
/// expression is not a plain assignment onto `temp`.
fn member_assignment_prop<'a>(expr: &'a Expr, temp: &BindingId) -> Option<&'a MemberProp> {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return None;
    };
    let Expr::Ident(object) = strip_parens(&member.obj) else {
        return None;
    };
    if !ident_matches_binding(object, temp) {
        return None;
    }
    Some(&member.prop)
}

fn seed_object_is_foldable(seed: &ObjectLit) -> bool {
    seed.props.iter().all(|prop| match prop {
        PropOrSpread::Spread(_) => true,
        PropOrSpread::Prop(prop) => match prop.as_ref() {
            Prop::Getter(_) | Prop::Setter(_) | Prop::Assign(_) => false,
            Prop::KeyValue(kv) => !prop_name_is_proto(&kv.key),
            Prop::Method(method) => !prop_name_is_proto(&method.key),
            Prop::Shorthand(ident) => ident.sym != PROTO,
        },
    })
}

fn prop_name_is_proto(name: &PropName) -> bool {
    match name {
        PropName::Ident(ident) => ident.sym == PROTO,
        PropName::Str(str_lit) => str_lit.value == PROTO,
        PropName::Computed(computed) => expr_is_static_proto_key(&computed.expr),
        PropName::Num(_) | PropName::BigInt(_) => false,
    }
}

fn expr_is_static_proto_key(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Lit(Lit::Str(str_lit)) => str_lit.value == PROTO,
        Expr::Tpl(template) if template.exprs.is_empty() && template.quasis.len() == 1 => {
            let Some(quasi) = template.quasis.first() else {
                return false;
            };
            quasi
                .cooked
                .as_ref()
                .map_or_else(|| quasi.raw == PROTO, |cooked| cooked == PROTO)
        }
        _ => false,
    }
}

fn assignment_to_prop(expr: &Expr, temp: &BindingId) -> Option<PropOrSpread> {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    key_value_prop(member_assignment_prop(expr, temp)?, &assign.right)
}

fn key_value_prop(prop: &MemberProp, value: &Expr) -> Option<PropOrSpread> {
    let key = member_prop_to_prop_name(prop)?;
    Some(PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
        key,
        value: Box::new(value.clone()),
    }))))
}

/// Pick the property-name form. Babel discards which keys were computed in the
/// source, so a key that is already a literal drops its brackets; anything
/// dynamic stays computed. String keys are left as `PropName::Str` because
/// `UnBracketNotation` runs later and already normalizes those to identifier or
/// numeric property names.
fn member_prop_to_prop_name(prop: &MemberProp) -> Option<PropName> {
    match prop {
        MemberProp::Ident(ident) => (ident.sym != PROTO).then(|| PropName::Ident(ident.clone())),
        MemberProp::PrivateName(_) => None,
        MemberProp::Computed(computed) => {
            if expr_is_static_proto_key(&computed.expr) {
                return None;
            }

            match strip_parens(&computed.expr) {
                Expr::Lit(Lit::Str(str_lit)) => Some(PropName::Str(str_lit.clone())),
                // A numeric literal coerces to the same string in either position.
                Expr::Lit(Lit::Num(num)) => Some(PropName::Num(num.clone())),
                other => Some(PropName::Computed(ComputedPropName {
                    span: computed.span,
                    expr: Box::new(other.clone()),
                })),
            }
        }
    }
}

fn count_expr_binding_refs(exprs: &[Box<Expr>], binding: &BindingId) -> usize {
    exprs
        .iter()
        .map(|expr| count_binding_refs(expr.as_ref(), binding))
        .sum()
}
