//! Name a short generated default or namespace import local, or a `require`
//! alias, after the module it holds:
//!
//! ```js
//! import * as Qc from "./module-221.js";      // → module221
//! import Ab from "./utils/date-format.js";    // → dateFormat
//! import Rk from "react";                     // → React
//! import * as Xa from "node:child_process";   // → childProcess
//! var n = require("./shared/base-codec.js");  // → baseCodec
//! ```
//!
//! A source path is the weakest naming evidence there is: it says which
//! module the binding holds, not what the code does with it. So this rule
//! runs after every pass that can name an import binding (value-position
//! renames in `SmartRename`/`SmartRename2`, `UnImportRename2`,
//! `UnExportRename2`, the JSX passes, and the names `UnEsm` takes from
//! provider facts in unpack mode). Unpack runs it on each finished module
//! instead of in the registry range: after the late ESM recovery and its
//! `SmartRename` pass, source-map names, and the recovered-filename remap, so
//! an import whose provider got its original filename back is named after
//! that filename, not its provisional `module-<id>` one. It touches only a
//! local that is still a short generated alias (`is_likely_generated_alias`),
//! except a `_` or `$` prefix on a three-letter word (`_map`), which earlier
//! rules derive. Named imports (`import { a as b }`) belong to
//! `UnImportRename`.
//!
//! # `require` aliases
//!
//! A `require` that `UnEsm` did not turn into an import (a module it could
//! not normalize, or a call inside a function) is named like a default import
//! of its source, without the `Namespace` form. The declarator must be
//! `<ident> = require("<string>")` with the global `require`, and the
//! binding must keep that value for its whole life: one declaration, no
//! direct write, no parameter of the same name (`collect_stable_aliases`). A
//! `var` belongs to its function-like scope wherever it sits; a `let` or
//! `const` counts only directly in a module or function body. Later rules do
//! not rename these declarators, so the names stand.
//!
//! # Candidate names
//!
//! Each source yields candidate names, tried in order. A candidate that is
//! not a valid identifier, or is reserved, is skipped; a `_N` suffix on the
//! first valid candidate comes only after every candidate is rejected.
//!
//! - A relative path names its file stem in camelCase, then prefixes the
//!   parent directories one level at a time, nearest first:
//!   `./shared/utils/date-format.js` → `dateFormat`, `utilsDateFormat`,
//!   `sharedUtilsDateFormat`. An `index` file takes its directory's name.
//!   Uninformative directories ([`is_uninformative_directory`]) are skipped,
//!   and JavaScript and TypeScript extensions are stripped.
//! - An unpacked module's provisional `module-<id>` stem names only itself
//!   (`./module-221.js` → `module221`): its directory says nothing about it.
//!   The `_<n>` that filename deduplication appends when several inputs emit
//!   one name stays (`./module-221_2.js` → `module221_2`).
//!
//! Words join in camelCase, except that two digit runs keep a `_` between
//! them (`./chunk-12_3.js` → `chunk12_3`, not `chunk123`), so the name never
//! reads like a different file.
//! - A Node builtin (`node:x`, or a bare `x` in [`NODE_BUILTIN_MODULES`])
//!   names the module, then the module with a `node` prefix: `path`,
//!   `nodePath`; `child_process` → `childProcess`, `nodeChildProcess`.
//! - A bare package names its last meaningful segment, then joins its
//!   parents in front, nearest first: `lodash/debounce` → `debounce`,
//!   `lodashDebounce`; `@tanstack/react-query` → `reactQuery`,
//!   `tanstackReactQuery`. A generic last segment ([`is_generic_segment`])
//!   does not stand alone: `react-dom/client` → `reactDomClient`,
//!   `@sentry/core` → `sentryCore`.
//! - [`conventional_package_name`] holds the few packages whose community
//!   spelling differs from the derived one (`react` → `React`).
//! - A readable name the module exports the binding under
//!   (`export { Qc as utils }`) comes before all of these.
//!
//! A local used as a bare JSX tag (`<Ab />`) takes every candidate in
//! PascalCase: a lowercase tag is an intrinsic element, not a reference. So
//! does a local of a relative source whose naming stem starts with an
//! uppercase letter, whatever its use: `./Wrapper.js` → `Wrapper`,
//! `./utils/LoginService.js` → `LoginService`, then `UtilsLoginService`, so
//! `new Wrapper()` and `extends LoginService` read as they were written. A
//! hash-like stem (`./Ab3x9QzK.js` → `Ab3x9QzK`) keeps its capital too; that
//! costs nothing.
//!
//! A namespace import of a module that the module also imports by default
//! (`node:` prefix ignored) takes `<base>Namespace` instead, whatever the
//! statement order and whether the default import is readable: `import React
//! from "react"` beside `import * as reactNamespace from "react"`,
//! `module7` beside `module7Namespace`. The directory prefixes would name the
//! same file, so the fallback is a `_N` suffix. Two default imports of one
//! source take the ordinary candidates.
//!
//! # Collisions
//!
//! A candidate is rejected only when the rename would change what some
//! identifier refers to. For a module-scope binding:
//!
//! 1. a module-level binding already has the name, or an earlier binding in
//!    this pass took it;
//! 2. the module has a free reference with that name (a global `process`),
//!    which a module binding would capture;
//! 3. a nested scope declares the name around a reference to this binding
//!    (`RenameShadowIndex`). A nested scope that declares the name but never
//!    reads the binding only shadows it and does not block the name.
//!
//! A `require` alias inside a function checks the same in its own scope: the
//! scope declares the name (a parameter, a hoisted `var` or function, a
//! body-level `let`/`const`/class, or an alias this pass named), or the scope
//! reads an outer binding or a free global spelled that way, or rule 3. Outer
//! bindings are read under their new names: imports and module-scope
//! bindings are named first, then nested aliases, outermost scope first.
//!
//! Export specifiers keep their public names through `BindingRenamer`:
//! `export { Qc }` becomes `export { module221 as Qc }`; an exported
//! declaration (`export var n = require(...)`) keeps its name. No later rule
//! renames a local back to its export name (`UnExportRename2` runs earlier).
//! A module with a `with` statement or a direct `eval` keeps its names
//! (dynamic-scope skip), and no binding is named `require` or another
//! CommonJS or global value name ([`is_forbidden_target`]).
//!
//! Member-init may have named `var t = p.version` `p_version` from the old
//! name; such a binding follows the new name (`module110_version`), as in the
//! type-name pass of `SmartRename`.

use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmt, Callee, CatchClause, ClassDecl, ClassExpr, Constructor, ExportSpecifier,
    Expr, FnDecl, FnExpr, ForInStmt, ForOfStmt, ForStmt, Function, Ident, ImportSpecifier, Lit,
    Module, ModuleDecl, ModuleExportName, ModuleItem, ParamOrTsParamProp, Pat, StaticBlock,
    SwitchStmt, TsParamPropParam, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMut, VisitWith};

use crate::js_names::{
    is_likely_generated_alias, is_reserved_binding_name, is_valid_identifier_name,
};

use super::eval_utils::has_dynamic_scope_construct;
use super::expr_utils::is_unresolved_ident;
use super::rename_utils::{
    collect_exported_binding_ids, collect_free_reference_names, collect_jsx_tag_bindings,
    collect_module_names, rename_bindings_in_module, starts_with_lowercase, BindingId,
    BindingRename, RenameShadowIndex,
};
use super::smart_rename::{
    collect_stable_aliases, lower_camel_type_name, rename_stale_member_prefixes,
};

/// How many `_N` suffixes to try on the first candidate before giving up.
const MAX_SUFFIX: usize = 100;

pub struct ImportSourceRename {
    unresolved_mark: Mark,
}

impl ImportSourceRename {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for ImportSourceRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let imports = collect_module_imports(module);
        let mut requires = collect_require_aliases(module, self.unresolved_mark);
        if (!imports.iter().any(|import| import.candidate) && requires.aliases.is_empty())
            || has_dynamic_scope_construct(module)
        {
            return;
        }

        let mut taken = collect_module_names(module);
        let free_names = collect_free_reference_names(module, self.unresolved_mark);
        let jsx_tags = collect_jsx_tag_bindings(module);
        let exported_names = collect_local_export_names(module);
        let exported_decls = collect_exported_binding_ids(module);

        // A namespace import of a module the module also imports by default
        // is named `<base>Namespace`, so the two bindings read apart.
        let default_sources: HashSet<&str> = imports
            .iter()
            .filter(|import| !import.namespace)
            .map(|import| import.module_key.as_str())
            .collect();

        // Imports first, then module-scope requires, then nested requires
        // outermost first: a nested binding checks the outer bindings its
        // scope reads under their new names.
        let mut targets: Vec<Target<'_>> = imports
            .iter()
            .filter(|import| import.candidate)
            .map(|import| Target {
                local: import.local.clone(),
                source: &import.source,
                namespace_name: import.namespace
                    && default_sources.contains(import.module_key.as_str()),
                frame: None,
            })
            .collect();
        targets.extend(
            requires
                .aliases
                .iter()
                .filter(|alias| !exported_decls.contains(&alias.local))
                .map(|alias| Target {
                    local: alias.local.clone(),
                    source: &alias.source,
                    namespace_name: false,
                    frame: alias.frame,
                }),
        );
        if targets.is_empty() {
            return;
        }
        let bindings: HashSet<BindingId> =
            targets.iter().map(|target| target.local.clone()).collect();
        let shadow_index = RenameShadowIndex::for_bindings(module, &bindings);

        let mut renames = Vec::new();
        let mut planned: HashMap<BindingId, Atom> = HashMap::default();
        for target in &targets {
            let local = &target.local;
            let jsx_tag = jsx_tags.contains(local);
            let case = if jsx_tag || has_pascal_case_stem(target.source) {
                NameCase::Pascal
            } else {
                NameCase::Camel
            };
            // `export { Qc as utils }` names the binding better than its
            // source does. An export under the local's own name
            // (`export { Qc }`) keeps its public name through an alias.
            let exports = exported_names.get(local).map(Vec::as_slice).unwrap_or(&[]);
            let mut candidates: Vec<String> = exports
                .iter()
                .filter(|name| {
                    name.as_str() != local.0.as_ref()
                        && is_usable_name(name)
                        && !is_likely_generated_alias(name)
                        && !(jsx_tag && starts_with_lowercase(name))
                })
                .cloned()
                .collect();
            let source_names = source_name_candidates(target.source, case);
            let source_names = if target.namespace_name {
                namespace_candidate(&source_names, case)
                    .into_iter()
                    .collect()
            } else {
                source_names
            };
            for name in source_names {
                if !candidates.contains(&name) {
                    candidates.push(name);
                }
            }
            if candidates.is_empty()
                || candidates
                    .iter()
                    .any(|name| name.as_str() == local.0.as_ref())
            {
                continue;
            }
            let frame = target
                .frame
                .map(|index| requires.frames[index].names_in_scope(&planned));
            let accepts = |name: &str| {
                let atom = Atom::from(name);
                let scope_free = match frame {
                    // A module-scope binding: no other module binding or
                    // free global may have the name.
                    None => !taken.contains(&atom) && !free_names.contains(&atom),
                    // A nested binding: its own scope must not declare the
                    // name, and nothing the scope reads from outside it may
                    // be spelled that way.
                    Some((declared, outer_names)) => {
                        !declared.contains(&atom) && !outer_names.contains(&atom)
                    }
                };
                scope_free && !shadow_index.rename_causes_shadowing(local, &atom)
            };
            let chosen = candidates
                .iter()
                .find(|name| accepts(name))
                .cloned()
                .or_else(|| {
                    let base = &candidates[0];
                    (1..=MAX_SUFFIX)
                        .map(|i| format!("{base}_{i}"))
                        .find(|name| accepts(name))
                });
            let Some(chosen) = chosen else {
                continue;
            };
            let chosen = Atom::from(chosen);
            match target.frame {
                None => {
                    taken.insert(chosen.clone());
                }
                Some(index) => {
                    requires.frames[index].declared.insert(chosen.clone());
                }
            }
            planned.insert(local.clone(), chosen.clone());
            renames.push(BindingRename {
                old: local.clone(),
                new: chosen,
            });
        }

        if renames.is_empty() {
            return;
        }
        rename_bindings_in_module(module, &renames);
        collapse_same_name_exports(module, &renames);
        // Member-init derived `p_version` from `var t = p.version` long before
        // `p` became `module110`.
        rename_stale_member_prefixes(module, &renames, &collect_exported_binding_ids(module));
    }
}

/// A binding this pass may name.
struct Target<'a> {
    local: BindingId,
    source: &'a str,
    /// A namespace import of a module also imported by default: it takes
    /// `<base>Namespace`.
    namespace_name: bool,
    /// The function-like scope that holds a nested `require` alias, as an
    /// index into [`RequireAliases::frames`]; `None` at module scope.
    frame: Option<usize>,
}

/// `BindingRenamer` keeps an export's public name with an alias; a binding
/// renamed to that very name (`export { effects as effects }`) needs none.
fn collapse_same_name_exports(module: &mut Module, renames: &[BindingRename]) {
    for item in &mut module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) = item else {
            continue;
        };
        if export.src.is_some() {
            continue;
        }
        for specifier in &mut export.specifiers {
            let ExportSpecifier::Named(named) = specifier else {
                continue;
            };
            let (ModuleExportName::Ident(orig), Some(ModuleExportName::Ident(exported))) =
                (&named.orig, &named.exported)
            else {
                continue;
            };
            if orig.sym == exported.sym
                && renames
                    .iter()
                    .any(|rename| rename.new == orig.sym && rename.old.1 == orig.ctxt)
            {
                named.exported = None;
            }
        }
    }
}

/// `react` → `reactNamespace`, `module7` → `module7Namespace`: the first
/// source candidate, lowered in camelCase, with a `Namespace` suffix. The
/// directory-prefixed candidates name the same file, so they are not tried.
fn namespace_candidate(source_names: &[String], case: NameCase) -> Option<String> {
    let base = source_names.first()?;
    let base = match case {
        NameCase::Camel => lower_camel_type_name(base),
        NameCase::Pascal => base.clone(),
    };
    let name = format!("{base}Namespace");
    is_usable_name(&name).then_some(name)
}

struct ModuleImport {
    local: BindingId,
    /// `import * as local`, not `import local`.
    namespace: bool,
    source: String,
    /// `source` without a `node:` prefix: `fs` and `node:fs` are one module.
    module_key: String,
    /// The local is still a short generated alias, so this rule may name it.
    candidate: bool,
}

fn module_key(source: &str) -> String {
    source.strip_prefix("node:").unwrap_or(source).to_string()
}

/// Every default and namespace import local, in source order.
fn collect_module_imports(module: &Module) -> Vec<ModuleImport> {
    let mut imports = Vec::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let Some(source) = import.src.value.as_str() else {
            continue;
        };
        for specifier in &import.specifiers {
            let (local, namespace) = match specifier {
                ImportSpecifier::Default(default) => (&default.local, false),
                ImportSpecifier::Namespace(namespace) => (&namespace.local, true),
                ImportSpecifier::Named(_) => continue,
            };
            imports.push(ModuleImport {
                local: (local.sym.clone(), local.ctxt),
                namespace,
                source: source.to_string(),
                module_key: module_key(source),
                candidate: is_generated_import_local(&local.sym),
            });
        }
    }
    imports
}

/// `is_likely_generated_alias`, except a `_` or `$` prefix on a
/// three-letter word (`_map`, `_dep`): earlier rules derive those names
/// from a member or a source, and no minifier reaches four-character names.
fn is_generated_import_local(name: &str) -> bool {
    if let Some(word) = name.strip_prefix('_').or_else(|| name.strip_prefix('$')) {
        if word.len() == 3 && word.chars().all(|c| c.is_ascii_alphabetic()) {
            return false;
        }
    }
    is_likely_generated_alias(name)
}

/// `require("<source>")` with the global `require` and one string argument.
fn require_source(init: &Expr, unresolved_mark: Mark) -> Option<&str> {
    let Expr::Call(call) = init else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(callee) = callee.as_ref() else {
        return None;
    };
    if !is_unresolved_ident(callee, "require", unresolved_mark) {
        return None;
    }
    let [arg] = call.args.as_slice() else {
        return None;
    };
    if arg.spread.is_some() {
        return None;
    }
    let Expr::Lit(Lit::Str(source)) = arg.expr.as_ref() else {
        return None;
    };
    source.value.as_str()
}

/// A short generated binding that holds `require("<source>")` for its whole
/// life.
struct RequireAlias {
    local: BindingId,
    source: String,
    /// The function-like scope that holds the binding, as an index into
    /// [`RequireAliases::frames`]; `None` at module scope.
    frame: Option<usize>,
}

struct RequireAliases {
    /// Module-scope aliases first, then nested ones, outermost scope first.
    aliases: Vec<RequireAlias>,
    frames: Vec<FrameNames>,
}

/// What a nested alias's new name must not collide with in the function-like
/// scope that holds it.
#[derive(Default)]
struct FrameNames {
    /// Names the scope itself declares: its parameters, its hoisted `var`s
    /// and function declarations, its body's lexical declarations, and the
    /// names this pass has already given its aliases.
    declared: HashSet<Atom>,
    /// Bindings the scope reads but does not declare: outer bindings and
    /// free globals. A same-named binding in the scope would capture them.
    outer_refs: HashSet<BindingId>,
    /// `outer_refs` spelled under their planned names, built on first use.
    /// Only module-scope targets and aliases of enclosing scopes, which this
    /// pass names first, can rename an outer binding.
    outer_names: Option<HashSet<Atom>>,
}

impl FrameNames {
    fn names_in_scope(
        &mut self,
        planned: &HashMap<BindingId, Atom>,
    ) -> (&HashSet<Atom>, &HashSet<Atom>) {
        let outer_refs = &self.outer_refs;
        let outer_names = self.outer_names.get_or_insert_with(|| {
            outer_refs
                .iter()
                .map(|id| planned.get(id).unwrap_or(&id.0).clone())
                .collect()
        });
        (&self.declared, outer_names)
    }
}

/// Every `var`/`let`/`const` binding that is still a short generated alias
/// and holds `require("<source>")` for its whole life: one declaration, no
/// direct write, and no parameter of the same name (`collect_stable_aliases`).
/// A `var` belongs to its function-like scope; a `let` or `const` counts
/// only directly in a module or function body, where its scope is that one.
fn collect_require_aliases(module: &Module, unresolved_mark: Mark) -> RequireAliases {
    let mut finder = ScopeWalker::new(unresolved_mark, None);
    module.visit_with(&mut finder);
    let found = finder.found;
    if found.is_empty() {
        return RequireAliases {
            aliases: Vec::new(),
            frames: Vec::new(),
        };
    }
    let stable = collect_stable_aliases(module, |init| {
        require_source(init, unresolved_mark).is_some()
    });
    let mut found: Vec<FoundAlias> = found
        .into_iter()
        .filter(|alias| stable.contains(&alias.local))
        .collect();
    // Module scope first, then outermost scopes first.
    found.sort_by_key(|alias| alias.frame.map_or(0, |(_, depth)| depth));

    let mut frame_index: HashMap<usize, usize> = HashMap::default();
    for alias in &found {
        if let Some((id, _)) = alias.frame {
            let next = frame_index.len();
            frame_index.entry(id).or_insert(next);
        }
    }
    let mut frames = Vec::new();
    if !frame_index.is_empty() {
        let mut walker = ScopeWalker::new(unresolved_mark, Some(&frame_index));
        module.visit_with(&mut walker);
        frames = walker
            .facts
            .into_iter()
            .map(|facts| FrameNames {
                declared: facts.declared,
                outer_refs: facts
                    .idents
                    .into_iter()
                    .filter(|id| !facts.inner_decls.contains(id))
                    .collect(),
                outer_names: None,
            })
            .collect();
    }
    let aliases = found
        .into_iter()
        .map(|alias| RequireAlias {
            local: alias.local,
            source: alias.source,
            frame: alias.frame.map(|(id, _)| frame_index[&id]),
        })
        .collect();
    RequireAliases { aliases, frames }
}

struct FoundAlias {
    local: BindingId,
    source: String,
    /// The holding function-like frame's walk id and its function depth.
    frame: Option<(usize, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Function,
    Block,
}

struct Frame {
    id: usize,
    kind: FrameKind,
    /// Index into `ScopeWalker::facts` when this frame is tracked.
    facts: Option<usize>,
}

#[derive(Default)]
struct FrameFacts {
    declared: HashSet<Atom>,
    idents: HashSet<BindingId>,
    /// Every binding declared anywhere inside the frame.
    inner_decls: HashSet<BindingId>,
}

/// Where a declaration puts its name.
#[derive(Clone, Copy)]
enum DeclScope {
    /// `var` and function declarations: the nearest function-like frame.
    Hoisted,
    /// `let`, `const`, classes, parameters: the innermost frame.
    Lexical,
    /// A named function or class expression binds its name in a scope of its
    /// own, inside no frame.
    Own,
}

/// Walks the module's scopes, numbering frames in visit order, so that two
/// walks agree on frame ids. Without tracked frames it finds `require`
/// aliases; with them it records each tracked frame's names.
struct ScopeWalker<'a> {
    unresolved_mark: Mark,
    stack: Vec<Frame>,
    next_id: usize,
    found: Vec<FoundAlias>,
    tracked: Option<&'a HashMap<usize, usize>>,
    facts: Vec<FrameFacts>,
    /// Tracked frames on the stack.
    active: Vec<usize>,
}

impl<'a> ScopeWalker<'a> {
    fn new(unresolved_mark: Mark, tracked: Option<&'a HashMap<usize, usize>>) -> Self {
        let facts = tracked
            .map(|tracked| {
                std::iter::repeat_with(FrameFacts::default)
                    .take(tracked.len())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            unresolved_mark,
            stack: Vec::new(),
            next_id: 0,
            found: Vec::new(),
            tracked,
            facts,
            active: Vec::new(),
        }
    }

    fn with_frame(&mut self, kind: FrameKind, visit: impl FnOnce(&mut Self)) {
        let id = self.next_id;
        self.next_id += 1;
        let facts = self.tracked.and_then(|tracked| tracked.get(&id).copied());
        if let Some(index) = facts {
            self.active.push(index);
        }
        self.stack.push(Frame { id, kind, facts });
        visit(self);
        self.stack.pop();
        if facts.is_some() {
            self.active.pop();
        }
    }

    fn declare(&mut self, ids: impl IntoIterator<Item = BindingId>, scope: DeclScope) {
        if self.active.is_empty() {
            return;
        }
        let owner = match scope {
            DeclScope::Hoisted => self
                .stack
                .iter()
                .rev()
                .find(|frame| frame.kind == FrameKind::Function)
                .and_then(|frame| frame.facts),
            DeclScope::Lexical => self.stack.last().and_then(|frame| frame.facts),
            DeclScope::Own => None,
        };
        for id in ids {
            for &index in &self.active {
                self.facts[index].inner_decls.insert(id.clone());
            }
            if let Some(owner) = owner {
                self.facts[owner].declared.insert(id.0.clone());
            }
        }
    }

    fn declare_pat(&mut self, pat: &Pat, scope: DeclScope) {
        if self.active.is_empty() {
            return;
        }
        let ids: Vec<BindingId> = find_pat_ids(pat);
        self.declare(ids, scope);
    }

    /// Record a `require` alias declarator (first walk only).
    fn find_alias(&mut self, kind: VarDeclKind, declarator: &VarDeclarator) {
        let (Pat::Ident(binding), Some(init)) = (&declarator.name, declarator.init.as_deref())
        else {
            return;
        };
        if !is_generated_import_local(&binding.id.sym) {
            return;
        }
        let Some(source) = require_source(init, self.unresolved_mark) else {
            return;
        };
        let frame = if kind == VarDeclKind::Var {
            self.stack
                .iter()
                .rev()
                .find(|frame| frame.kind == FrameKind::Function)
        } else {
            match self.stack.last() {
                Some(frame) if frame.kind == FrameKind::Function => Some(frame),
                Some(_) => return,
                None => None,
            }
        };
        let frame = frame.map(|frame| {
            let depth = self
                .stack
                .iter()
                .take_while(|outer| outer.id != frame.id)
                .filter(|outer| outer.kind == FrameKind::Function)
                .count()
                + 1;
            (frame.id, depth)
        });
        self.found.push(FoundAlias {
            local: (binding.id.sym.clone(), binding.id.ctxt),
            source: source.to_string(),
            frame,
        });
    }
}

impl Visit for ScopeWalker<'_> {
    fn visit_function(&mut self, function: &Function) {
        self.with_frame(FrameKind::Function, |this| {
            for param in &function.params {
                this.declare_pat(&param.pat, DeclScope::Lexical);
            }
            function.visit_children_with(this);
        });
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.with_frame(FrameKind::Function, |this| {
            for param in &arrow.params {
                this.declare_pat(param, DeclScope::Lexical);
            }
            arrow.visit_children_with(this);
        });
    }

    fn visit_constructor(&mut self, ctor: &Constructor) {
        self.with_frame(FrameKind::Function, |this| {
            for param in &ctor.params {
                match param {
                    ParamOrTsParamProp::Param(param) => {
                        this.declare_pat(&param.pat, DeclScope::Lexical);
                    }
                    ParamOrTsParamProp::TsParamProp(prop) => match &prop.param {
                        TsParamPropParam::Ident(binding) => this.declare(
                            [(binding.id.sym.clone(), binding.id.ctxt)],
                            DeclScope::Lexical,
                        ),
                        TsParamPropParam::Assign(assign) => {
                            this.declare_pat(&assign.left, DeclScope::Lexical);
                        }
                    },
                }
            }
            ctor.visit_children_with(this);
        });
    }

    // A static block is a function-like scope for its `var`s; its statements
    // are its body, not a nested block.
    fn visit_static_block(&mut self, block: &StaticBlock) {
        self.with_frame(FrameKind::Function, |this| {
            block.body.visit_children_with(this)
        });
    }

    fn visit_block_stmt(&mut self, block: &BlockStmt) {
        self.with_frame(FrameKind::Block, |this| block.visit_children_with(this));
    }

    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        self.with_frame(FrameKind::Block, |this| stmt.visit_children_with(this));
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        self.with_frame(FrameKind::Block, |this| stmt.visit_children_with(this));
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        self.with_frame(FrameKind::Block, |this| stmt.visit_children_with(this));
    }

    fn visit_switch_stmt(&mut self, stmt: &SwitchStmt) {
        stmt.discriminant.visit_with(self);
        self.with_frame(FrameKind::Block, |this| stmt.cases.visit_with(this));
    }

    fn visit_catch_clause(&mut self, catch: &CatchClause) {
        self.with_frame(FrameKind::Block, |this| {
            if let Some(param) = &catch.param {
                this.declare_pat(param, DeclScope::Lexical);
            }
            catch.visit_children_with(this);
        });
    }

    fn visit_var_decl(&mut self, declaration: &VarDecl) {
        let scope = if declaration.kind == VarDeclKind::Var {
            DeclScope::Hoisted
        } else {
            DeclScope::Lexical
        };
        for declarator in &declaration.decls {
            if self.tracked.is_none() {
                self.find_alias(declaration.kind, declarator);
            }
            self.declare_pat(&declarator.name, scope);
        }
        declaration.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, declaration: &FnDecl) {
        let ident = &declaration.ident;
        // Hoisted: a function declared in a block also binds in its
        // function in sloppy mode.
        self.declare([(ident.sym.clone(), ident.ctxt)], DeclScope::Hoisted);
        declaration.function.visit_with(self);
    }

    fn visit_class_decl(&mut self, declaration: &ClassDecl) {
        let ident = &declaration.ident;
        self.declare([(ident.sym.clone(), ident.ctxt)], DeclScope::Lexical);
        declaration.class.visit_with(self);
    }

    fn visit_fn_expr(&mut self, expr: &FnExpr) {
        if let Some(ident) = &expr.ident {
            self.declare([(ident.sym.clone(), ident.ctxt)], DeclScope::Own);
        }
        expr.function.visit_with(self);
    }

    fn visit_class_expr(&mut self, expr: &ClassExpr) {
        if let Some(ident) = &expr.ident {
            self.declare([(ident.sym.clone(), ident.ctxt)], DeclScope::Own);
        }
        expr.class.visit_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        for &index in &self.active {
            self.facts[index]
                .idents
                .insert((ident.sym.clone(), ident.ctxt));
        }
    }
}

/// The names under which `export { local as name }` (without `from`) makes
/// each local public. `default` is not a name a binding can take.
fn collect_local_export_names(module: &Module) -> HashMap<BindingId, Vec<String>> {
    let mut names: HashMap<BindingId, Vec<String>> = HashMap::default();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) = item else {
            continue;
        };
        if export.src.is_some() {
            continue;
        }
        for specifier in &export.specifiers {
            let ExportSpecifier::Named(named) = specifier else {
                continue;
            };
            let ModuleExportName::Ident(orig) = &named.orig else {
                continue;
            };
            let exported = match &named.exported {
                None => Some(orig.sym.to_string()),
                Some(ModuleExportName::Ident(ident)) => Some(ident.sym.to_string()),
                Some(ModuleExportName::Str(str)) => str.value.as_str().map(str::to_string),
            };
            if let Some(exported) = exported.filter(|name| name != "default") {
                names
                    .entry((orig.sym.clone(), orig.ctxt))
                    .or_default()
                    .push(exported);
            }
        }
    }
    names
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NameCase {
    Camel,
    Pascal,
}

/// The candidate names for an import of `source`, in the order they are
/// tried. Every name is a valid, unreserved identifier.
pub(crate) fn source_name_candidates(source: &str, case: NameCase) -> Vec<String> {
    // Already PascalCase, so a JSX tag can take it too.
    if let Some(name) = conventional_package_name(source) {
        return vec![name.to_string()];
    }
    let mut names = Vec::new();
    for words in source_word_candidates(source) {
        let name = join_words(&words, case);
        if is_usable_name(&name) && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn is_usable_name(name: &str) -> bool {
    !name.is_empty()
        && is_valid_identifier_name(name)
        && !is_reserved_binding_name(name)
        && !is_forbidden_target(name)
}

/// Names an import never takes even when the module does not reference
/// them: CommonJS module-scope names, which a `require` read in a direct
/// `eval` can still reach (`has_dynamic_scope_construct` lets that eval
/// through), and global values whose shadowing misleads a reader.
fn is_forbidden_target(name: &str) -> bool {
    matches!(
        name,
        "require" | "module" | "exports" | "undefined" | "NaN" | "Infinity" | "globalThis"
    )
}

/// The module path of `source`, without a query or hash and with `/`
/// separators. A loader prefix (`raw-loader!./x`) or a URL is not a module
/// path.
fn module_path(source: &str) -> Option<String> {
    if source.contains('!') || source.contains("://") || source.starts_with("data:") {
        return None;
    }
    let source = source
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .replace('\\', "/");
    (!source.is_empty()).then_some(source)
}

fn is_relative_path(path: &str) -> bool {
    path.starts_with('.') || path.starts_with('/')
}

/// A relative source whose naming stem starts with an uppercase letter
/// (`./Wrapper.js`, `./utils/LoginService.js`) names a class or a component
/// by convention, so every candidate keeps PascalCase, used as a JSX tag or
/// not.
fn has_pascal_case_stem(source: &str) -> bool {
    module_path(source).is_some_and(|path| {
        is_relative_path(&path)
            && relative_naming_segments(&path)
                .is_some_and(|(stem, _)| stem.starts_with(|c: char| c.is_ascii_uppercase()))
    })
}

/// Candidate names as word lists. A word list joins into one identifier in
/// the requested case.
fn source_word_candidates(source: &str) -> Vec<Vec<String>> {
    let Some(source) = module_path(source) else {
        return Vec::new();
    };
    if is_relative_path(&source) {
        return relative_word_candidates(&source);
    }
    if let Some(builtin) = source.strip_prefix("node:") {
        return builtin_word_candidates(builtin);
    }
    let first_segment = source.split('/').next().unwrap_or_default();
    if NODE_BUILTIN_MODULES.contains(&first_segment) {
        return builtin_word_candidates(&source);
    }
    package_word_candidates(&source)
}

/// `./shared/utils/date-format.js` → `[date, format]`, `[utils, date,
/// format]`, `[shared, utils, date, format]`.
fn relative_word_candidates(path: &str) -> Vec<Vec<String>> {
    let Some((stem, segments)) = relative_naming_segments(path) else {
        return Vec::new();
    };
    if let Some((id, collision)) = provisional_module_stem(stem) {
        let mut words = vec!["module".to_string(), id.to_string()];
        // `_2` stays a separate word, so `module-aB3x_2` cannot read as
        // `module-aB3x2`.
        if let Some(collision) = collision {
            words.push(format!("_{collision}"));
        }
        return vec![words];
    }

    let mut candidates = vec![split_words(stem)];
    let mut words = split_words(stem);
    for parent in segments.iter().rev() {
        if is_uninformative_directory(parent) {
            continue;
        }
        let mut prefixed = split_words(parent);
        prefixed.extend(words);
        words = prefixed;
        candidates.push(words.clone());
    }
    candidates
}

/// The segment that names a relative path, and the directories above it:
/// the file stem without its script extension, or for an `index` file or a
/// directory import the nearest informative directory.
fn relative_naming_segments(path: &str) -> Option<(&str, Vec<&str>)> {
    let directory_index = path.ends_with('/');
    let mut segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect();
    if !directory_index {
        let file = segments.pop()?;
        let stem = strip_script_extension(file);
        if stem != "index" {
            segments.push(stem);
        }
    }
    // An `index` file, or a directory import, takes the name of the nearest
    // informative directory.
    while segments
        .last()
        .is_some_and(|segment| is_uninformative_directory(segment))
    {
        segments.pop();
    }
    let stem = segments.pop()?;
    Some((stem, segments))
}

/// The provisional name an unpacker gives a module without a recovered path:
/// `module-221` for a numeric id, `module-<id>` for a short string id. When
/// several inputs emit the same name, filename deduplication appends `_<n>`
/// (`module-221_2`). Returns the id and that collision number.
fn provisional_module_stem(stem: &str) -> Option<(&str, Option<&str>)> {
    let rest = stem.strip_prefix("module-")?;
    let (id, collision) = match rest.rsplit_once('_') {
        Some((id, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => (id, Some(n)),
        _ => (rest, None),
    };
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())).then_some((id, collision))
}

/// `child_process` → `[child, process]`, then `[node, child, process]`.
fn builtin_word_candidates(name: &str) -> Vec<Vec<String>> {
    let words: Vec<String> = name
        .split('/')
        .flat_map(|segment| split_words(strip_script_extension(segment)))
        .collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut prefixed = vec!["node".to_string()];
    prefixed.extend(words.iter().cloned());
    vec![words, prefixed]
}

/// `@tanstack/react-query` → `[react, query]`, `[tanstack, react, query]`.
fn package_word_candidates(source: &str) -> Vec<Vec<String>> {
    let mut segments: Vec<&str> = source.split('/').filter(|s| !s.is_empty()).collect();
    if let Some(scope) = segments.first_mut() {
        *scope = scope.trim_start_matches('@');
    }
    if segments.len() > 1 {
        if let Some(last) = segments.last_mut() {
            *last = strip_script_extension(last);
        }
    }
    // The package name itself always counts; only its subpath can hold
    // build directories (`date-fns/esm/format`) or an `index` file.
    let package_len = if source.starts_with('@') { 2 } else { 1 };
    let parts: Vec<&str> = segments
        .iter()
        .enumerate()
        .filter(|(index, segment)| {
            *index < package_len || !(is_uninformative_directory(segment) || **segment == "index")
        })
        .map(|(_, segment)| *segment)
        .collect();
    let Some(last) = parts.last() else {
        return Vec::new();
    };

    let first_len = if parts.len() > 1 && is_generic_segment(last) {
        2
    } else {
        1
    };
    (first_len..=parts.len())
        .map(|len| {
            parts[parts.len() - len..]
                .iter()
                .flat_map(|part| split_words(part))
                .collect()
        })
        .collect()
}

/// Packages whose community spelling differs from the derived name. Keep
/// this to names nearly every codebase spells the same way.
fn conventional_package_name(source: &str) -> Option<&'static str> {
    match source {
        "react" => Some("React"),
        "react-dom" => Some("ReactDOM"),
        _ => None,
    }
}

/// Build and layout directories that say nothing about the module they hold.
fn is_uninformative_directory(segment: &str) -> bool {
    matches!(
        segment,
        "." | ".."
            | "src"
            | "lib"
            | "dist"
            | "build"
            | "out"
            | "es"
            | "esm"
            | "cjs"
            | "umd"
            | "node_modules"
    )
}

/// Package subpath or name segments too generic to name an import alone:
/// a runtime target (`react-dom/client`, `@sentry/browser`), a build or
/// layout directory, an entry point, or a framework adapter
/// (`@emotion/react` is not React).
fn is_generic_segment(segment: &str) -> bool {
    matches!(
        segment,
        "promises"
            | "index"
            | "main"
            | "core"
            | "client"
            | "server"
            | "browser"
            | "node"
            | "web"
            | "native"
            | "shared"
            | "common"
            | "utils"
            | "util"
            | "types"
            | "runtime"
            | "esm"
            | "cjs"
            | "dist"
            | "lib"
            | "react"
            | "vue"
    )
}

/// Node.js builtin modules (`module.builtinModules`, without the internal
/// `_`-prefixed ones). Only `node:`-prefixed specifiers reach the builtins
/// that have no bare form (`node:test`, `node:sqlite`), so they are not
/// listed. A subpath (`fs/promises`) is a builtin when its first segment is.
pub(crate) const NODE_BUILTIN_MODULES: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

const SCRIPT_EXTENSIONS: &[&str] = &[".js", ".mjs", ".cjs", ".jsx", ".ts", ".tsx", ".mts", ".cts"];

fn strip_script_extension(file: &str) -> &str {
    SCRIPT_EXTENSIONS
        .iter()
        .find_map(|extension| file.strip_suffix(extension).filter(|stem| !stem.is_empty()))
        .unwrap_or(file)
}

/// Split a path segment into words at every character that cannot appear
/// in an identifier word: `date-format` → `[date, format]`,
/// `lodash.debounce` → `[lodash, debounce]`.
fn split_words(segment: &str) -> Vec<String> {
    segment
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Join words in camelCase (`dateFormat`, a leading acronym lowered as one
/// word: `URLParser` → `urlParser`) or PascalCase (`DateFormat`). Two digit
/// runs keep a `_` between them, so `chunk-12_3` → `chunk12_3` does not read
/// as `chunk-123`.
fn join_words(words: &[String], case: NameCase) -> String {
    let mut name = String::new();
    for (index, word) in words.iter().enumerate() {
        if name.ends_with(|c: char| c.is_ascii_digit())
            && word.starts_with(|c: char| c.is_ascii_digit())
        {
            name.push('_');
        }
        if index == 0 && case == NameCase::Camel {
            name.push_str(&lower_camel_type_name(word));
        } else {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                name.extend(first.to_uppercase());
                name.push_str(chars.as_str());
            }
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camel(source: &str) -> Vec<String> {
        source_name_candidates(source, NameCase::Camel)
    }

    fn pascal(source: &str) -> Vec<String> {
        source_name_candidates(source, NameCase::Pascal)
    }

    #[test]
    fn relative_paths_name_the_stem_then_prefix_parents() {
        assert_eq!(
            camel("./shared/utils/date-format.js"),
            ["dateFormat", "utilsDateFormat", "sharedUtilsDateFormat"]
        );
        assert_eq!(
            camel("../widgets/color_picker.tsx"),
            ["colorPicker", "widgetsColorPicker"]
        );
        assert_eq!(camel("./src/lib/parser.mjs"), ["parser"]);
        assert_eq!(camel("./api/URLParser.ts"), ["urlParser", "apiURLParser"]);
        assert_eq!(camel("./styles.css"), ["stylesCss"]);
        assert_eq!(camel("./logo.svg?url"), ["logoSvg"]);
    }

    #[test]
    fn an_index_file_takes_its_directory_name() {
        assert_eq!(
            camel("./components/button/index.js"),
            ["button", "componentsButton"]
        );
        assert_eq!(camel("./widgets/src/index.js"), ["widgets"]);
        assert_eq!(camel("./widgets/"), ["widgets"]);
        assert!(camel("./index.js").is_empty());
        assert!(camel("../src/index.js").is_empty());
        assert!(camel("..").is_empty());
    }

    #[test]
    fn provisional_module_names_take_no_directory_prefix() {
        assert_eq!(camel("./module-221.js"), ["module221"]);
        assert_eq!(camel("../chunks/module-12.js"), ["module12"]);
        assert_eq!(camel("./module-aB3x.js"), ["moduleAB3x"]);
        assert_eq!(pascal("./module-12.js"), ["Module12"]);
    }

    #[test]
    fn a_deduplicated_provisional_name_keeps_its_collision_suffix() {
        assert_eq!(camel("./module-221_2.js"), ["module221_2"]);
        assert_eq!(camel("../chunks/module-12_3.js"), ["module12_3"]);
        assert_eq!(camel("./module-aB3x_2.js"), ["moduleAB3x_2"]);
        assert_eq!(pascal("./module-12_2.js"), ["Module12_2"]);
    }

    #[test]
    fn adjacent_digit_runs_keep_a_separator() {
        assert_eq!(camel("./chunk-12_3.js"), ["chunk12_3"]);
        assert_eq!(camel("./v2/3d.js"), ["v2_3d"]);
        assert_eq!(camel("./parts-4-5-6.js"), ["parts4_5_6"]);
        assert_eq!(camel("./render/3d-utils.js"), ["render3dUtils"]);
        assert_eq!(camel("./v2-utils.js"), ["v2Utils"]);
    }

    #[test]
    fn invalid_or_reserved_stems_fall_through_to_prefixed_names() {
        assert_eq!(camel("./parts/class.js"), ["partsClass"]);
        assert_eq!(camel("./render/3d-utils.js"), ["render3dUtils"]);
        assert!(camel("./3d-utils.js").is_empty());
        assert!(camel("./class.js").is_empty());
        assert_eq!(camel("./loaders/require.js"), ["loadersRequire"]);
    }

    #[test]
    fn packages_name_the_last_meaningful_segment_then_join_parents() {
        assert_eq!(camel("lodash/debounce"), ["debounce", "lodashDebounce"]);
        assert_eq!(camel("lodash.debounce"), ["lodashDebounce"]);
        assert_eq!(
            camel("@tanstack/react-query"),
            ["reactQuery", "tanstackReactQuery"]
        );
        assert_eq!(
            camel("date-fns/esm/format/index.js"),
            ["format", "dateFnsFormat"]
        );
        assert_eq!(camel("socket.io-client"), ["socketIoClient"]);
        assert_eq!(camel("classnames"), ["classnames"]);
    }

    #[test]
    fn generic_last_segments_start_with_the_joined_name() {
        assert_eq!(camel("react-dom/client"), ["reactDomClient"]);
        assert_eq!(camel("@sentry/core"), ["sentryCore"]);
        assert_eq!(camel("@emotion/react"), ["emotionReact"]);
        assert_eq!(
            camel("@apollo/client/react"),
            ["clientReact", "apolloClientReact"]
        );
        assert_eq!(camel("some-lib/utils"), ["someLibUtils"]);
    }

    #[test]
    fn node_builtins_name_the_module_then_add_a_node_prefix() {
        for source in ["path", "node:path"] {
            assert_eq!(camel(source), ["path", "nodePath"]);
        }
        for source in ["child_process", "node:child_process"] {
            assert_eq!(camel(source), ["childProcess", "nodeChildProcess"]);
        }
        assert_eq!(camel("fs/promises"), ["fsPromises", "nodeFsPromises"]);
        assert_eq!(camel("node:fs/promises"), ["fsPromises", "nodeFsPromises"]);
        assert_eq!(camel("node:test"), ["test", "nodeTest"]);
        assert_eq!(pascal("node:path"), ["Path", "NodePath"]);
        // `module` is a CommonJS name.
        assert_eq!(camel("node:module"), ["nodeModule"]);
    }

    #[test]
    fn a_stem_with_a_leading_capital_is_pascal_case() {
        for source in [
            "./Wrapper.js",
            "./utils/LoginService.js",
            "./Header/index.js",
            "./Ad.js",
            "./Ab3x9QzK.js",
            "./URLParser.ts",
        ] {
            assert!(has_pascal_case_stem(source), "{source}");
        }
        for source in [
            "./wrapper.js",
            "./module-12.js",
            "some-kit/Wrapper",
            "./9Ab.js",
        ] {
            assert!(!has_pascal_case_stem(source), "{source}");
        }
    }

    #[test]
    fn convention_table_spells_community_names() {
        assert_eq!(camel("react"), ["React"]);
        assert_eq!(camel("react-dom"), ["ReactDOM"]);
        assert_eq!(pascal("react"), ["React"]);
    }

    #[test]
    fn pascal_case_capitalizes_every_candidate() {
        assert_eq!(
            pascal("./shared/utils/date-format.js"),
            ["DateFormat", "UtilsDateFormat", "SharedUtilsDateFormat"]
        );
        assert_eq!(
            pascal("@tanstack/react-query"),
            ["ReactQuery", "TanstackReactQuery"]
        );
    }

    #[test]
    fn loader_and_url_sources_have_no_candidates() {
        assert!(camel("raw-loader!./notes.txt").is_empty());
        assert!(camel("https://cdn.example.com/lib.js").is_empty());
        assert!(camel("").is_empty());
    }
}
