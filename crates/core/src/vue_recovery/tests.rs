use super::*;
use crate::vue_template::{VueAttr, VueExpr};

mod attrs_events;
mod computed;
mod helper_aliases;
mod recognition;
mod ref_values;
mod script_setup;
mod selection;
mod template;

fn test_stmt(source: &str) -> Stmt {
    let cm = Lrc::new(SourceMap::default());
    let module = parse_module(source, cm).unwrap();
    match module.body.into_iter().next().unwrap() {
        ModuleItem::Stmt(stmt) => stmt,
        _ => panic!("expected statement"),
    }
}

/// Parse `source` and run `resolver()` over it (mirroring the recovery entry
/// points) so statements carry real `SyntaxContext`s, then return the top-level
/// statements.
fn resolved_stmts(source: &str) -> Vec<Stmt> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_module(source, cm).unwrap();
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        module
            .body
            .into_iter()
            .filter_map(|item| match item {
                ModuleItem::Stmt(stmt) => Some(stmt),
                _ => None,
            })
            .collect()
    })
}

fn primed_context(source: &str) -> VueRecoveryContext {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_module(source, cm.clone()).unwrap();
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        let mut ctx = collect_context(&module, cm, HashMap::new(), HashMap::new());
        ctx.unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
        let render = find_render_source(&module, None).expect("render source");
        prime_render_context(render, &mut ctx).unwrap();
        ctx
    })
}

fn test_atoms(names: &[&str]) -> Vec<Atom> {
    names.iter().map(|name| Atom::from(*name)).collect()
}

fn test_atom_set(names: &[&str]) -> HashSet<Atom> {
    names.iter().map(|name| Atom::from(*name)).collect()
}

fn recover_source_with_imports<F>(source: &str, resolve_import: F) -> Result<Option<String>>
where
    F: FnMut(&str) -> Option<String>,
{
    recover_vue_sfc_source_from_js(
        source,
        VueSfcRecoveryOptions::default().with_import_resolver(resolve_import),
    )
}

fn decompile_sfc(source: &str, decompile: DecompileOptions) -> Result<DecompileOutput> {
    Ok(decompile_vue_sfc(source, VueSfcDecompileOptions::new(decompile))?.output)
}

fn decompile_sfc_with_imports<F>(
    source: &str,
    decompile: DecompileOptions,
    resolve_import: F,
) -> Result<DecompileOutput>
where
    F: FnMut(&str) -> Option<String>,
{
    Ok(decompile_vue_sfc(
        source,
        VueSfcDecompileOptions {
            decompile,
            recovery: VueSfcRecoveryOptions::default().with_import_resolver(resolve_import),
        },
    )?
    .output)
}

fn test_local_binding(
    source: &str,
    bindings: &[&str],
    emitted_bindings: &[&str],
    refs: &[&str],
) -> VueSetupLocalBinding {
    test_local_binding_with_scope(source, bindings, emitted_bindings, refs, false)
}

fn test_local_binding_with_scope(
    source: &str,
    bindings: &[&str],
    emitted_bindings: &[&str],
    refs: &[&str],
    module_scope: bool,
) -> VueSetupLocalBinding {
    VueSetupLocalBinding {
        bindings: test_atoms(bindings),
        emitted_bindings: test_atoms(emitted_bindings),
        refs: test_atom_set(refs),
        source: source.to_string(),
        import_refs: HashSet::new(),
        stmt: test_stmt(source),
        module_scope,
        template_selectable: true,
        setup_order: 0,
        always_emit: false,
        preserve_ref_values: false,
    }
}
