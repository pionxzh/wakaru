mod flip_comparisons;
mod remove_void;
mod simplify_sequence;
mod un_infinity;
mod unminify_booleans;
mod un_numeric_literal;
mod un_typeof;

use swc_core::ecma::ast::Module;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub use flip_comparisons::FlipComparisons;
pub use remove_void::RemoveVoid;
pub use simplify_sequence::SimplifySequence;
pub use un_infinity::UnInfinity;
pub use unminify_booleans::UnminifyBooleans;
pub use un_numeric_literal::UnNumericLiteral;
pub use un_typeof::UnTypeof;

pub trait Rule: VisitMut {
    fn name(&self) -> &'static str;
}

#[derive(Default)]
pub struct NoopRule;

impl VisitMut for NoopRule {}

impl Rule for NoopRule {
    fn name(&self) -> &'static str {
        "noop"
    }
}

pub fn apply_default_rules(module: &mut Module) {
    module.visit_mut_with(&mut SimplifySequence);
    module.visit_mut_with(&mut FlipComparisons);
    if RemoveVoid::should_run(module) {
        module.visit_mut_with(&mut RemoveVoid);
    }
    module.visit_mut_with(&mut UnminifyBooleans);
    module.visit_mut_with(&mut UnInfinity);
    module.visit_mut_with(&mut UnTypeof);
    module.visit_mut_with(&mut UnNumericLiteral);
}
