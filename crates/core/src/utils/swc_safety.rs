use std::panic::{self, AssertUnwindSafe};

use anyhow::anyhow;
use swc_core::ecma::ast::Module;
use swc_core::ecma::transforms::base::fixer::fixer;
use swc_core::ecma::visit::VisitMutWith;

/// Run SWC's fixer pass, catching panics from malformed AST that the
/// error-recovery parser accepted but the fixer doesn't handle.
pub(crate) fn apply_fixer(module: &mut Module) -> anyhow::Result<()> {
    panic::catch_unwind(AssertUnwindSafe(|| {
        module.visit_mut_with(&mut fixer(None));
    }))
    .map_err(|payload| {
        let msg = payload
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("unknown panic");
        anyhow!("SWC fixer panicked on malformed AST: {msg}")
    })
}
