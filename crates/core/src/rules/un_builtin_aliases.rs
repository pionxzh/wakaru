use swc_core::common::Mark;
use swc_core::ecma::ast::Module;
use swc_core::ecma::visit::VisitMut;

use super::builtin_aliases::{inline_module_builtin_aliases, BuiltinAliasInlineOptions};

pub struct UnBuiltinAliases {
    unresolved_mark: Option<Mark>,
}

impl UnBuiltinAliases {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_mark: Some(unresolved_mark),
        }
    }

    pub(crate) fn run(&mut self, module: &mut Module) -> bool {
        inline_module_builtin_aliases(
            module,
            self.unresolved_mark,
            BuiltinAliasInlineOptions::early_var_aliases(),
        )
    }
}

impl VisitMut for UnBuiltinAliases {
    fn visit_mut_module(&mut self, module: &mut Module) {
        self.run(module);
    }
}
