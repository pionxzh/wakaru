use std::fmt;
use std::sync::Arc;

use crate::error::{Error, ErrorKind, Result};
use crate::Source;

pub trait ImportResolver: Send + Sync {
    fn resolve(&self, specifier: &str) -> Option<String>;
}

impl<F> ImportResolver for F
where
    F: Fn(&str) -> Option<String> + Send + Sync,
{
    fn resolve(&self, specifier: &str) -> Option<String> {
        self(specifier)
    }
}

#[derive(Clone, Default)]
pub struct RecoveryOptions {
    preferred_component_name: Option<String>,
    import_resolver: Option<Arc<dyn ImportResolver>>,
}

impl fmt::Debug for RecoveryOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryOptions")
            .field("preferred_component_name", &self.preferred_component_name)
            .field("has_import_resolver", &self.import_resolver.is_some())
            .finish()
    }
}

impl RecoveryOptions {
    pub fn preferred_component_name(&self) -> Option<&str> {
        self.preferred_component_name.as_deref()
    }

    pub fn with_preferred_component_name(mut self, name: impl Into<String>) -> Self {
        self.preferred_component_name = Some(name.into());
        self
    }

    pub fn with_import_resolver(mut self, resolver: impl ImportResolver + 'static) -> Self {
        self.import_resolver = Some(Arc::new(resolver));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecoveredSfc {
    pub name: Option<String>,
    pub source: String,
}

pub fn recover(input: Source, options: RecoveryOptions) -> Result<Vec<RecoveredSfc>> {
    let input = input.into_parts();
    if input.source_map.is_some() {
        return Err(Error::new(
            ErrorKind::InvalidOptions,
            Some(input.filename),
            anyhow::anyhow!("standalone Vue recovery does not accept an input source map"),
        ));
    }

    let resolver = options.import_resolver;
    let mut core_options = wakaru_core::VueSfcRecoveryOptions::default();
    if let Some(name) = options.preferred_component_name.as_deref() {
        core_options = core_options.with_preferred_component_name(name);
    }
    if let Some(resolver) = resolver {
        core_options =
            core_options.with_import_resolver(move |specifier| resolver.resolve(specifier));
    }

    wakaru_core::recover_vue_sfcs_from_js(&input.code, core_options)
        .map(|recovered| {
            recovered
                .into_iter()
                .map(|sfc| RecoveredSfc {
                    name: sfc.name,
                    source: sfc.sfc.print(),
                })
                .collect()
        })
        .map_err(|error| Error::new(ErrorKind::Parse, Some(input.filename), error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_non_vue_module_returns_no_artifacts() {
        let recovered = recover(
            Source::new("plain.js", "export const value = 1;"),
            RecoveryOptions::default(),
        )
        .expect("valid JavaScript should recover successfully");
        assert!(recovered.is_empty());
    }

    #[test]
    fn recovery_options_own_the_name_and_resolver() {
        let name = String::from("OwnedComponent");
        let options = RecoveryOptions::default()
            .with_preferred_component_name(name)
            .with_import_resolver(|_: &str| None);
        assert_eq!(options.preferred_component_name(), Some("OwnedComponent"));
    }
}
