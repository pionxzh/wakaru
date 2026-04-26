//! Re-export consolidation: redirect imports from passthrough modules to the
//! actual target module.
//!
//! A passthrough module has the shape `export default require("./X.js")` — it
//! re-exports another module's namespace as its default export. Importing from
//! a passthrough is semantically equivalent to a namespace import from the
//! target: `import x from "./passthrough.js"` becomes
//! `import * as x from "./target.js"`.
//!
//! This runs at the Stage 2 barrier, before namespace decomposition. The
//! resulting namespace imports can then be further decomposed into named imports
//! by the namespace decomposition pass.

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    Ident, ImportSpecifier, ImportStarAsSpecifier, ModuleDecl, ModuleItem, Module, Str,
};

use crate::facts::ModuleFactsMap;

pub fn run_reexport_consolidation(module: &mut Module, module_facts: &ModuleFactsMap) {
    if module_facts.is_empty() {
        return;
    }

    for item in &mut module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };

        if import.specifiers.len() != 1 {
            continue;
        }
        let ImportSpecifier::Default(default_spec) = &import.specifiers[0] else {
            continue;
        };

        let source_str = import.src.value.as_str().unwrap_or("");
        let Some(target) = resolve_passthrough(source_str, module_facts) else {
            continue;
        };

        let local_ident = default_spec.local.clone();
        import.specifiers = vec![ImportSpecifier::Namespace(ImportStarAsSpecifier {
            span: DUMMY_SP,
            local: Ident::new(local_ident.sym, DUMMY_SP, local_ident.ctxt),
        })];
        import.src = Box::new(Str::from(target.as_ref()));
    }
}

fn resolve_passthrough(source: &str, facts: &ModuleFactsMap) -> Option<Atom> {
    let mut current = source.to_string();
    let mut seen = std::collections::HashSet::new();

    loop {
        if !seen.insert(current.clone()) {
            return None;
        }
        let module_facts = facts.get(&current)?;
        let target = module_facts.passthrough_target.as_ref()?;
        let target_str = target.as_ref();

        if let Some(target_facts) = facts.get(target_str) {
            if target_facts.passthrough_target.is_some() {
                current = target_str.to_string();
                continue;
            }
        }
        return Some(target.clone());
    }
}
