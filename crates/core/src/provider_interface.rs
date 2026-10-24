//! What a provider module offers the modules that required it, read from its
//! own [`ModuleFacts`].
//!
//! Unpack decides a module's format in Phase 1, where every module runs
//! alone, so a consumer's `require("./p")` becomes an import before `./p` is
//! known. After the fact barrier, the Phase 2 passes ask the provider what
//! that require returned and which import form still reads it. Those
//! provider-side answers live here, derived once from the provider's facts;
//! what the consumer did with the require (its interop wrappers, whole-value
//! uses) stays with each pass.

use swc_core::atoms::Atom;

use crate::facts::{ExportKind, ModuleFacts};

/// What `require("./p")` returned while the provider was CommonJS, and where
/// the provider's recovered form holds that value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequiredValue {
    /// The provider stays CommonJS: `require` still returns its
    /// `module.exports`, and so does a default import of it.
    CommonJs,
    /// `module.exports = v`, now only `export default v`: the value is the
    /// namespace's `default`. `marked`: the module also marked itself
    /// `__esModule`, so a `.default` read of it already meant that default
    /// (a Babel runtime shim's `module.exports.default = module.exports`, or
    /// the self mirror `v.default = v` of Next's default-object tail).
    Default { marked: bool },
    /// `module.exports = v` plus named exports, which are on the namespace,
    /// not on `v`: a call or `new` reads `default`, a named export stays a
    /// namespace read.
    DefaultWithNamed,
    /// The module marked itself `__esModule`. Its namespace is its exports
    /// object, and Node adds the `__esModule` marker to the namespace only
    /// when the module has a default export (`require(esm)`, since v22.12
    /// and v20.19), so only then does an interop that tests it still read
    /// the default.
    Marked { has_default: bool },
    /// `module.exports` replaced another way: the value is unknown.
    Replaced,
    /// The `exports` object, unmarked and never replaced: the namespace holds
    /// the same names.
    ExportsObject,
}

/// How an ES module importer reads the value `require("./p")` returned when
/// `./p` was CommonJS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequiredValueImport {
    /// The default import: a provider that stays CommonJS (its
    /// `module.exports`), or one whose CommonJS value is now its only
    /// default export (`module.exports = v`, or a value that also marked
    /// itself `__esModule`, such as Next's default-object tail).
    Default,
    /// The namespace import, read through members only: an ES module, or a
    /// provider that marked itself `__esModule`, whose namespace holds its
    /// exports.
    Namespace,
}

/// The interface one provider offers its consumers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderInterface<'a> {
    facts: &'a ModuleFacts,
    /// What `require` of the provider returned.
    pub value: RequiredValue,
    /// The recovered module has a default export.
    pub has_default: bool,
    /// The recovered module has a named export, or an `export *` whose
    /// names come from another module.
    pub has_named_surface: bool,
}

impl<'a> ProviderInterface<'a> {
    pub(crate) fn of(facts: &'a ModuleFacts) -> Self {
        let has_default = facts
            .exports
            .iter()
            .any(|export| export.kind == ExportKind::Default);
        let has_named_surface = facts.has_export_all
            || facts
                .exports
                .iter()
                .any(|export| export.kind == ExportKind::Named);
        // `export *` and re-exports leave names this module cannot list.
        let closed_surface = !facts.has_export_all && facts.reexports.is_empty();
        let value = if facts.stays_commonjs {
            RequiredValue::CommonJs
        } else if facts.commonjs_value_is_default && has_default && closed_surface {
            if facts.exports.len() == 1 {
                RequiredValue::Default {
                    marked: facts.marks_es_module,
                }
            } else {
                RequiredValue::DefaultWithNamed
            }
        } else if facts.marks_es_module {
            RequiredValue::Marked { has_default }
        } else if facts.replaces_module_exports {
            RequiredValue::Replaced
        } else {
            RequiredValue::ExportsObject
        };
        Self {
            facts,
            value,
            has_default,
            has_named_surface,
        }
    }

    pub(crate) fn is_named_export(&self, name: &str) -> bool {
        self.facts
            .exports
            .iter()
            .any(|export| export.kind == ExportKind::Named && export.exported == *name)
    }

    /// The module marked its `exports` `__esModule` before `UnEsm` removed
    /// the marker, whatever its recovered form.
    pub(crate) fn marks_es_module(&self) -> bool {
        self.facts.marks_es_module
    }

    /// Named exports whose function reads `this`.
    pub(crate) fn receiver_observing_exports(&self) -> &'a [Atom] {
        &self.facts.receiver_observing_exports
    }

    /// The import that reads the value `require` returned, `None` when no
    /// import holds it: the provider's `module.exports` was replaced another
    /// way, or `v` sits beside named exports.
    pub(crate) fn required_value_import(&self) -> Option<RequiredValueImport> {
        match self.value {
            RequiredValue::CommonJs | RequiredValue::Default { .. } => {
                Some(RequiredValueImport::Default)
            }
            RequiredValue::ExportsObject | RequiredValue::Marked { .. } => {
                Some(RequiredValueImport::Namespace)
            }
            RequiredValue::DefaultWithNamed | RequiredValue::Replaced => None,
        }
    }

    /// `require` of the recovered ES module returns its namespace, and that
    /// namespace is the value `require` returned before: an exports object
    /// or a marked module.
    pub(crate) fn require_returns_namespace(&self) -> bool {
        matches!(
            self.value,
            RequiredValue::ExportsObject | RequiredValue::Marked { .. }
        )
    }

    /// Whether a namespace import reads what a consumer's default import of
    /// this provider meant. `whole_value` says the consumer used the
    /// required value whole, or through an interop that returns it as is:
    /// that value is the namespace only when `require` returned the
    /// `exports` object or the namespace of a module the input wrote as ESM,
    /// where a default export is one property among the others. Without a
    /// default export the namespace is the only surface.
    pub(crate) fn namespace_holds_default_import(&self, whole_value: bool) -> bool {
        (self.has_named_surface || self.has_default)
            && (!self.has_default
                || ((self.facts.require_returns_exports_object || self.facts.input_is_es_module)
                    && whole_value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{ExportFact, ReexportFact};

    fn export(name: &str, kind: ExportKind) -> ExportFact {
        ExportFact {
            exported: name.into(),
            local: Some(name.into()),
            kind,
        }
    }

    fn default_only() -> ModuleFacts {
        ModuleFacts {
            exports: vec![export("default", ExportKind::Default)],
            ..Default::default()
        }
    }

    fn named_only() -> ModuleFacts {
        ModuleFacts {
            exports: vec![export("alpha", ExportKind::Named)],
            ..Default::default()
        }
    }

    fn default_and_named() -> ModuleFacts {
        ModuleFacts {
            exports: vec![
                export("default", ExportKind::Default),
                export("alpha", ExportKind::Named),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn required_value_follows_the_commonjs_facts() {
        use RequiredValue::*;
        let cases: Vec<(&str, ModuleFacts, RequiredValue)> = vec![
            (
                "stays CommonJS, whatever else holds",
                ModuleFacts {
                    stays_commonjs: true,
                    commonjs_value_is_default: true,
                    marks_es_module: true,
                    ..default_only()
                },
                CommonJs,
            ),
            (
                "module.exports = v alone",
                ModuleFacts {
                    commonjs_value_is_default: true,
                    replaces_module_exports: true,
                    ..default_only()
                },
                Default { marked: false },
            ),
            (
                "module.exports = v, also marked",
                ModuleFacts {
                    commonjs_value_is_default: true,
                    marks_es_module: true,
                    ..default_only()
                },
                Default { marked: true },
            ),
            (
                "module.exports = v beside named exports",
                ModuleFacts {
                    commonjs_value_is_default: true,
                    marks_es_module: true,
                    ..default_and_named()
                },
                DefaultWithNamed,
            ),
            (
                "module.exports = v with export *: the surface is open",
                ModuleFacts {
                    commonjs_value_is_default: true,
                    has_export_all: true,
                    replaces_module_exports: true,
                    ..default_only()
                },
                Replaced,
            ),
            (
                "module.exports = v with a re-export: the surface is open",
                ModuleFacts {
                    commonjs_value_is_default: true,
                    marks_es_module: true,
                    reexports: vec![ReexportFact {
                        exported: "beta".into(),
                        imported: "beta".into(),
                        source: "./beta.js".into(),
                    }],
                    ..default_only()
                },
                Marked { has_default: true },
            ),
            (
                "module.exports = v recovered without a default export",
                ModuleFacts {
                    commonjs_value_is_default: true,
                    ..named_only()
                },
                ExportsObject,
            ),
            (
                "marked, named only",
                ModuleFacts {
                    marks_es_module: true,
                    replaces_module_exports: true,
                    ..named_only()
                },
                Marked { has_default: false },
            ),
            (
                "marked, with a default",
                ModuleFacts {
                    marks_es_module: true,
                    ..default_and_named()
                },
                Marked { has_default: true },
            ),
            (
                "module.exports replaced another way",
                ModuleFacts {
                    replaces_module_exports: true,
                    ..default_and_named()
                },
                Replaced,
            ),
            ("the exports object", named_only(), ExportsObject),
            (
                "the exports object, proven",
                ModuleFacts {
                    require_returns_exports_object: true,
                    ..default_and_named()
                },
                ExportsObject,
            ),
        ];
        for (name, facts, expected) in cases {
            assert_eq!(ProviderInterface::of(&facts).value, expected, "{name}");
        }
    }

    #[test]
    fn required_value_import_reads_the_value_require_returned() {
        use RequiredValue::*;
        for (value, import, namespace) in [
            (CommonJs, Some(RequiredValueImport::Default), false),
            (
                Default { marked: false },
                Some(RequiredValueImport::Default),
                false,
            ),
            (
                Default { marked: true },
                Some(RequiredValueImport::Default),
                false,
            ),
            (DefaultWithNamed, None, false),
            (
                Marked { has_default: false },
                Some(RequiredValueImport::Namespace),
                true,
            ),
            (
                Marked { has_default: true },
                Some(RequiredValueImport::Namespace),
                true,
            ),
            (Replaced, None, false),
            (ExportsObject, Some(RequiredValueImport::Namespace), true),
        ] {
            let facts = ModuleFacts::default();
            let interface = ProviderInterface {
                value,
                ..ProviderInterface::of(&facts)
            };
            assert_eq!(interface.required_value_import(), import, "{value:?}");
            assert_eq!(
                interface.require_returns_namespace(),
                namespace,
                "{value:?}"
            );
        }
    }

    #[test]
    fn namespace_holds_a_default_import_without_a_whole_value_default() {
        let exports_object = |facts: ModuleFacts| ModuleFacts {
            require_returns_exports_object: true,
            ..facts
        };
        for (name, facts, whole_value, expected) in [
            ("no export at all", ModuleFacts::default(), true, false),
            ("named only", named_only(), false, true),
            (
                "export * only",
                ModuleFacts {
                    has_export_all: true,
                    ..Default::default()
                },
                false,
                true,
            ),
            (
                "a default from module.exports = v",
                default_only(),
                true,
                false,
            ),
            (
                "a default property of the exports object, read whole",
                exports_object(default_and_named()),
                true,
                true,
            ),
            (
                "a default property of the exports object, read as the default",
                exports_object(default_and_named()),
                false,
                false,
            ),
            (
                "a default export of a module the input wrote as ESM, read whole",
                ModuleFacts {
                    input_is_es_module: true,
                    ..default_and_named()
                },
                true,
                true,
            ),
            (
                "a default export of a module the input wrote as ESM, read as the default",
                ModuleFacts {
                    input_is_es_module: true,
                    ..default_and_named()
                },
                false,
                false,
            ),
        ] {
            assert_eq!(
                ProviderInterface::of(&facts).namespace_holds_default_import(whole_value),
                expected,
                "{name}"
            );
        }
    }
}
