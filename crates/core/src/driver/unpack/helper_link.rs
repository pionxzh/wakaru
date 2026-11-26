//! Lazy-factory helpers that one input imports from another.
//!
//! A code-split esbuild, Bun, or rolldown chunk imports `__commonJS` and
//! `__esm` from a shared chunk instead of declaring them, so its own
//! detection finds no factory. Each input is detected on arrival, before its
//! siblings are known. At that point it records the exports it proves to be
//! lazy helpers, and an input that calls a name imported from a relative path
//! in factory position keeps its source. Once every input is known,
//! [`link_imported_lazy_helpers`] resolves that input's imports against the
//! other inputs and detects it again with the names imported from a proven
//! helper export treated as helpers (`unpacker::esbuild::imported_helpers`).
//! The new detection replaces the first only when it is an esbuild/Bun
//! bundle; otherwise the input stays as it was detected first.

use std::path::{Path, PathBuf};

use crate::collections::HashMap;
use crate::unpacker::{
    with_imported_helpers, BundleFormat, FactoryPositionImport, ImportedLazyHelpers,
    LazyHelperKind, LazyHelperLinkFacts,
};

use super::super::types::{UnpackWarning, UnpackWarningKind};
use super::super::DriverResult;
use super::{
    prepare_unpack_input_with_policy, PreparedInputDetection, PreparedUnpackInput, ScopeHoistPolicy,
};

/// The proven lazy-helper exports of one input, by its filename.
#[derive(Debug, Clone)]
pub struct InputHelperExports {
    pub(super) filename: String,
    pub(super) exports: Vec<(String, LazyHelperKind)>,
}

/// Settings the input was first prepared with, reused when it is prepared
/// again.
#[derive(Clone, Copy)]
pub(super) struct PrepareSettings {
    pub(super) prepare_plain_ast: bool,
    pub(super) scope_hoist_policy: ScopeHoistPolicy,
    pub(super) output_source_maps: bool,
}

pub(super) struct PendingHelperLink {
    source: String,
    imports: Vec<FactoryPositionImport>,
    settings: PrepareSettings,
}

/// Records `facts` on a freshly prepared input. An input that imports a
/// factory-position name keeps `link_source` for detection again, unless it
/// is a bundle another detector claimed before esbuild's runs: imported
/// helpers cannot change that result.
pub(super) fn attach(
    input: &mut PreparedUnpackInput,
    facts: LazyHelperLinkFacts,
    link_source: Option<String>,
    settings: PrepareSettings,
) {
    input.lazy_helper_exports = facts.exports;
    let may_become_esbuild = match input.detection {
        PreparedInputDetection::Plain | PreparedInputDetection::ScopeHoisted => true,
        PreparedInputDetection::Bundle(format) => {
            matches!(format, BundleFormat::Esbuild | BundleFormat::ScopeHoisted)
        }
    };
    if let Some(source) = link_source.filter(|_| may_become_esbuild) {
        input.helper_link = Some(Box::new(PendingHelperLink {
            source,
            imports: facts.factory_imports,
            settings,
        }));
    }
}

/// Detects `input` again when it imports lazy-factory helpers that another
/// input in `inputs` proves, and returns a warning for each import whose
/// relative path names none of `inputs`.
///
/// `inputs` lists every input of the run, the given one included, so an
/// import of an input the caller skipped still resolves. A name counts as a
/// helper only when the input its specifier resolves to exports it as a
/// proven helper; how this input calls it proves nothing. The input is
/// replaced only when the new detection is an esbuild/Bun bundle. Either way
/// it no longer keeps its source for linking. An input that waits for no link
/// is left unchanged.
pub fn link_imported_lazy_helpers(
    input: &mut PreparedUnpackInput,
    inputs: &[InputHelperExports],
) -> DriverResult<Vec<UnpackWarning>> {
    let Some(link) = input.helper_link.take() else {
        return Ok(Vec::new());
    };
    let keys: Vec<String> = inputs
        .iter()
        .map(|other| input_key(Path::new(&other.filename)))
        .collect();

    let mut helpers: ImportedLazyHelpers = HashMap::default();
    let mut unresolved: Vec<(&str, Vec<&str>)> = Vec::new();
    for import in &link.imports {
        let target = resolve_relative(&input.filename, &import.specifier);
        let Some(exporter) = keys
            .iter()
            .position(|key| *key == target)
            .map(|index| &inputs[index])
        else {
            match unresolved
                .iter_mut()
                .find(|(specifier, _)| *specifier == import.specifier)
            {
                Some((_, locals)) => locals.push(&import.local),
                None => unresolved.push((&import.specifier, vec![&import.local])),
            }
            continue;
        };
        if let Some((_, kind)) = exporter
            .exports
            .iter()
            .find(|(name, _)| *name == import.imported)
        {
            helpers
                .entry(import.specifier.clone())
                .or_default()
                .insert(import.imported.as_str().into(), *kind);
        }
    }

    let warnings = unresolved
        .into_iter()
        .map(|(specifier, locals)| {
            let names = locals
                .iter()
                .map(|local| format!("`{local}`"))
                .collect::<Vec<_>>()
                .join(", ");
            UnpackWarning::new(
                input.filename.clone(),
                UnpackWarningKind::LazyHelperImportUnresolved,
                format!(
                    "{names} imported from {specifier:?} is called like a lazy module helper \
                     (`__commonJS`, `__esm`), but {specifier:?} is not among the inputs, so \
                     nothing proves it is one and the input is not split by it; pass that file \
                     in the same run"
                ),
            )
        })
        .collect();
    if helpers.is_empty() {
        return Ok(warnings);
    }

    let settings = link.settings;
    let mut linked = with_imported_helpers(helpers, || {
        prepare_unpack_input_with_policy(
            input.filename.clone(),
            link.source,
            settings.prepare_plain_ast,
            settings.scope_hoist_policy,
            settings.output_source_maps,
        )
    })?;
    if linked.detection == PreparedInputDetection::Bundle(BundleFormat::Esbuild) {
        linked.helper_link = None;
        *input = linked;
    }
    Ok(warnings)
}

/// The key of the input that `specifier`, imported from `importer`, names.
fn resolve_relative(importer: &str, specifier: &str) -> String {
    let parent = Path::new(importer).parent().unwrap_or(Path::new(""));
    input_key(&parent.join(specifier))
}

/// An input filename as an absolute, lexically normalized path, compared
/// without case, as output paths are.
fn input_key(path: &Path) -> String {
    let absolute: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    crate::driver::filename_case_key(
        &super::merge::normalize_path_lexically(&absolute)
            .to_string_lossy()
            .replace('\\', "/"),
    )
}
