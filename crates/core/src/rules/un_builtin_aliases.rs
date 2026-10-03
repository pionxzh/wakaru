use swc_core::common::Mark;
use swc_core::ecma::ast::Module;
use swc_core::ecma::visit::VisitMut;

use super::builtin_aliases::{inline_module_builtin_aliases, BuiltinAliasInlineOptions};
use super::un_esm::collect_cjs_export_getter_local_keys;

/// Replaces module-scope aliases of stable builtins (`var e = Object.freeze`)
/// with the builtin itself. Bundler runtimes emit such aliases, for example
/// esbuild's `__defProp = Object.defineProperty`; helper detection and the
/// structural recovery after it match the canonical `Object.defineProperty(...)`
/// call, so this runs before them (docs/helper-detection.md).
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
        // UnEsm runs next and turns each CommonJS export getter that returns a
        // local into an ESM export of that binding. Inlining the alias first
        // would leave the getter returning the global itself, and ESM cannot
        // export a binding the module does not declare.
        //
        // Keeping the declaration needs no snapshot-versus-live assumption.
        // In webpack, esbuild, and rollup output an export always names a
        // declared binding (webpack rejects `export { console as X }` at
        // parse time), so a getter that returns a global appears only after
        // this rule has inlined the alias.
        let pinned = self
            .unresolved_mark
            .map(|mark| collect_cjs_export_getter_local_keys(module, mark))
            .unwrap_or_default();
        inline_module_builtin_aliases(
            module,
            self.unresolved_mark,
            BuiltinAliasInlineOptions::early_var_aliases(),
            &pinned,
        )
    }
}

impl VisitMut for UnBuiltinAliases {
    fn visit_mut_module(&mut self, module: &mut Module) {
        self.run(module);
    }
}
