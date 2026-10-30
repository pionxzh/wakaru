//! Authored top-level code around a bundle's runtime statement: a raw banner
//! or footer, or scripts concatenated with the bundle. It runs before or after
//! the bundle, so unpackers keep it verbatim in entry.js, in source order
//! around the startup, instead of writing it to no output file. When a module
//! factory reads a name the code before the bundle declares, that code goes
//! to a prelude file instead, which entry.js loads first and which exports
//! the name to each module that reads it (see [`PreludeNames`]).

use std::cell::RefCell;

use crate::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, AssignExpr, AssignPatProp, AssignTarget, BindingIdent, CallExpr,
    Callee, Class, ClassDecl, Decl, ExportNamedSpecifier, ExportSpecifier, Expr, ExprStmt, FnDecl,
    ForHead, ForInStmt, ForOfStmt, Function, GetterProp, Ident, ImportDecl, KeyValueProp, Lit,
    MemberProp, Module, ModuleDecl, ModuleExportName, ModuleItem, NamedExport, ObjectPat,
    ObjectPatProp, Pat, Prop, PropName, PropOrSpread, SetterProp, SimpleAssignTarget, Stmt,
    UnaryExpr, UnaryOp, UpdateExpr, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::module_path::relative_import_specifier;
use crate::rules::eval_utils::{has_dynamic_scope_construct, node_has_dynamic_scope_construct};
use crate::rules::rename_utils::{rename_bindings_in_module, BindingRename};
use crate::unpacker::chunk_prelude::declares_top_level;
use crate::unpacker::{
    emit_module_with_source_map, resolve_prepared_module, spans_byte_ranges, UnpackedModule,
};
use crate::utils::paren::strip_parens;

/// The file the code before a bootstrap moves to when a factory reads one of
/// its declarations.
pub(super) const PRELUDE_FILENAME: &str = "prelude.js";

/// The input's top-level items before and after the bundle statement.
#[derive(Clone, Copy, Default)]
pub(super) struct SurroundingItems<'a> {
    before: &'a [ModuleItem],
    bundle: Option<&'a ModuleItem>,
    after: &'a [ModuleItem],
}

impl<'a> SurroundingItems<'a> {
    /// The items around `body[idx]`, the bundle statement.
    pub(super) fn around(body: &'a [ModuleItem], idx: usize) -> Self {
        Self {
            before: &body[..idx],
            bundle: Some(&body[idx]),
            after: &body[idx + 1..],
        }
    }
}

/// Code around the bundle statement, with the names it uses.
#[derive(Default)]
pub(super) struct SurroundingCode {
    before: Vec<ModuleItem>,
    after: Vec<ModuleItem>,
    /// Names the code reads as globals or declares at its top level. Moved
    /// next to them, entry bindings with these names would capture them.
    names: HashSet<Atom>,
    /// Every identifier name in the code, which renamed entry bindings avoid.
    all_names: HashSet<Atom>,
    /// Set when a factory reads a declaration of the code before the bundle:
    /// that code then goes to [`PRELUDE_FILENAME`] instead of entry.js.
    prelude: Option<PreludeNames>,
}

impl SurroundingCode {
    /// `None` when the code declares one of `entry_free_names` at its top
    /// level: entry.js gives those free names a meaning of its own (the
    /// module's `require`), and the declaration would capture them. Also
    /// `None` when a factory in `factory_spans` (inside the bundle statement)
    /// uses a name the code declares at its top level and
    /// [`PreludeNames::plan`] cannot bind it.
    pub(super) fn collect(
        items: SurroundingItems<'_>,
        entry_free_names: &[&str],
        factory_spans: &[Span],
    ) -> Option<Self> {
        // Directives and empty statements alone are no reason to write
        // entry.js.
        let is_inert = |item: &ModuleItem| match item {
            ModuleItem::Stmt(Stmt::Empty(_)) => true,
            ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) => {
                matches!(&**expr, Expr::Lit(Lit::Str(_)))
            }
            _ => false,
        };
        if items.before.iter().chain(items.after).all(is_inert) {
            return Some(Self::default());
        }

        let names = SurroundingNameCollector::collect(items.before.iter().chain(items.after));
        if entry_free_names
            .iter()
            .any(|name| names.top_level_names.contains(&Atom::from(*name)))
        {
            return None;
        }
        let before: Vec<&ModuleItem> = items.before.iter().collect();
        let after: Vec<&ModuleItem> = items.after.iter().collect();
        let bundle: Vec<&ModuleItem> = items.bundle.into_iter().collect();
        let prelude = PreludeNames::plan(&before, &after, &bundle, factory_spans, true)?;

        Some(Self {
            before: items.before.to_vec(),
            after: items.after.to_vec(),
            names: names.names,
            all_names: names.all_names,
            prelude,
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.before.is_empty() && self.after.is_empty()
    }

    /// The spans of the code entry.js holds.
    pub(super) fn spans(&self) -> impl Iterator<Item = Span> + '_ {
        let before: &[ModuleItem] = if self.prelude.is_some() {
            &[]
        } else {
            &self.before
        };
        before.iter().chain(&self.after).map(Spanned::span)
    }

    /// Give the factory module `module`, emitted as `filename`, a binding for
    /// each name of the prelude it reads. `require_ctxt` is the context of the
    /// module's free `require`. `None` when a read cannot be bound; see
    /// [`PreludeNames::bind`].
    pub(super) fn bind_factory(
        &self,
        module: &mut Module,
        require_ctxt: SyntaxContext,
        filename: &str,
    ) -> Option<()> {
        let Some(prelude) = &self.prelude else {
            return Some(());
        };
        let specifier = relative_import_specifier(filename, PRELUDE_FILENAME);
        prelude
            .bind_factory_reads(module, require_ctxt, &specifier, false)
            .map(|_| ())
    }

    /// Whether the code before the bundle goes to [`PRELUDE_FILENAME`].
    pub(super) fn has_prelude(&self) -> bool {
        self.prelude.is_some()
    }

    /// Rename the resolved entry `module`'s top-level bindings whose names the
    /// code uses. They were bundle-scope locals; next to the code, neither
    /// side may capture the other.
    pub(super) fn rename_conflicts(&self, module: &mut Module, top_level_mark: Mark) {
        if self.is_empty() {
            return;
        }
        let top_level_ctxt = SyntaxContext::empty().apply_mark(top_level_mark);
        let mut bindings = ContextSymCollector::new(top_level_ctxt);
        module.visit_with(&mut bindings);
        let mut conflicts: Vec<Atom> = bindings
            .syms
            .into_iter()
            .filter(|sym| self.names.contains(sym))
            .collect();
        if conflicts.is_empty() {
            return;
        }
        conflicts.sort_unstable();
        let mut taken = IdentNameCollector::default();
        module.visit_with(&mut taken);
        let mut taken = taken.0;
        taken.extend(self.all_names.iter().cloned());
        let renames: Vec<BindingRename> = conflicts
            .into_iter()
            .map(|sym| {
                let mut suffix = 1;
                let mut new = Atom::from(format!("{sym}_{suffix}"));
                while taken.contains(&new) {
                    suffix += 1;
                    new = Atom::from(format!("{sym}_{suffix}"));
                }
                taken.insert(new.clone());
                BindingRename {
                    old: (sym, top_level_ctxt),
                    new,
                }
            })
            .collect();
        rename_bindings_in_module(module, &renames);
    }

    /// The entry file `filename` for bundles whose startup only runs entry
    /// modules: the code before the bundle statement, a `require` of each
    /// entry module in startup order, and the code after it.
    pub(super) fn entry_module(
        &self,
        filename: &str,
        entry_filenames: &[&str],
        cm: &Lrc<SourceMap>,
    ) -> Option<UnpackedModule> {
        let mut module = Module {
            span: DUMMY_SP,
            body: entry_filenames
                .iter()
                .map(|entry| {
                    let specifier = relative_import_specifier(filename, entry);
                    ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                        span: DUMMY_SP,
                        expr: Box::new(require_call(SyntaxContext::empty(), &specifier)),
                    }))
                })
                .collect(),
            shebang: None,
        };
        self.place_around(&mut module, filename)?;
        let (code, generated_source_map) = emit_module_with_source_map(&module, cm.clone()).ok()?;
        Some(UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code,
            filename: filename.to_string(),
            source_ranges: spans_byte_ranges(cm, self.spans()),
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map,
            verbatim_source_offset: None,
            mapped_in_every_mode: false,
        })
    }

    /// Place the code before and after the entry `module`'s statements. With
    /// a prelude, the code before is replaced by a load of the prelude that
    /// binds the names entry.js reads from it. `None` when such a read cannot
    /// be bound.
    pub(super) fn place_around(&self, module: &mut Module, filename: &str) -> Option<()> {
        if self.is_empty() {
            return Some(());
        }
        let entry_items = std::mem::take(&mut module.body);
        let before: &[ModuleItem] = if self.prelude.is_some() {
            &[]
        } else {
            &self.before
        };
        module.body = before
            .iter()
            .cloned()
            .chain(entry_items)
            .chain(self.after.iter().cloned())
            .collect();
        if let Some(prelude) = &self.prelude {
            let specifier = relative_import_specifier(filename, PRELUDE_FILENAME);
            prelude.bind(
                module,
                &prelude.declared,
                None,
                SyntaxContext::empty(),
                &specifier,
                true,
            )?;
        }
        Some(())
    }

    /// Add the prelude file to `modules`, after every module that binds its
    /// names was given its binding. `Some(false)` when there is no prelude;
    /// `None` when another module already has its name.
    pub(super) fn push_prelude(
        &self,
        modules: &mut Vec<UnpackedModule>,
        cm: &Lrc<SourceMap>,
    ) -> Option<bool> {
        let Some(prelude) = &self.prelude else {
            return Some(false);
        };
        if modules
            .iter()
            .any(|module| module.filename.eq_ignore_ascii_case(PRELUDE_FILENAME))
        {
            return None;
        }
        let items: Vec<&ModuleItem> = self.before.iter().collect();
        modules.push(prelude_module(
            &items,
            prelude.exported(),
            PRELUDE_FILENAME,
            cm,
        )?);
        Some(true)
    }
}

/// Names that the code before a bundle statement declares at its top level
/// and that module files read, bound to a prelude file that holds that code.
///
/// The input is a classic script: a top-level declaration is a global binding,
/// and a module factory reads it as a free name (a banner's
/// `var version = 4;` or configuration object, or the helpers esbuild declares
/// when it minifies the bundle again: producer `esbuild@0.25.0 --minify
/// --keep-names` over webpack output). Split, the declaration is
/// module-scoped in the prelude, so the prelude exports it and each module
/// that reads it gets `var { name } = require("./prelude.js")`, which `UnEsm`
/// makes `import { name } from "./prelude.js"` when the module becomes ESM.
///
/// The code before the bundle runs to its end before any factory, so a value
/// it leaves is the value every factory reads, as long as nothing writes the
/// binding later. [`Self::plan`] proves that, so a module that stays CommonJS
/// and copies the value when it loads the prelude reads the same value as
/// the live ESM binding.
pub(super) struct PreludeNames {
    /// Names of the code before the bundle that a factory reads.
    factory_reads: HashSet<Atom>,
    /// The spans of those reads. A factory module's free identifier of one
    /// of the names but at another span was something else in the input: a
    /// Browserify factory's fourth parameter, which normalization drops.
    factory_read_spans: HashSet<Span>,
    /// Every top-level name the code before the bundle declares; entry.js
    /// binds the ones it reads.
    declared: HashSet<Atom>,
    /// Declared names the code before the bundle writes inside a function,
    /// which may run after a module copied the value.
    unstable: HashSet<Atom>,
    /// Read names that are esbuild's keep-names helper (see
    /// [`find_name_helpers`]), never reassigned.
    name_helpers: HashSet<Atom>,
    /// Names a module was given a binding for; the prelude exports them.
    bound: RefCell<HashSet<Atom>>,
}

/// Names a prelude never binds: the free names unpacking gives a meaning of
/// its own (the module's `require`, `module`, and `exports`, and `global` and
/// `define`, from webpack's `require.g` and `require.amdO`), and names a
/// strict-mode binding cannot take, since the prelude and its importers are
/// ES modules.
const UNBINDABLE_NAMES: [&str; 17] = [
    "require",
    "module",
    "exports",
    "global",
    "define",
    "eval",
    "arguments",
    "await",
    "yield",
    "let",
    "static",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
];

impl PreludeNames {
    /// Decide how the code around the `bundle` statements (`before` and
    /// `after` it) stays bound to the factories in `factory_spans`:
    /// `Some(None)` when no factory uses a name the code declares at its top
    /// level, `Some(Some(names))` when every such use is a read that a
    /// prelude holding the code before the bundle can bind, and `None` (the
    /// input stays one file) otherwise:
    ///
    /// - a factory uses a name the code after the bundle declares or writes,
    ///   or writes a declared name;
    /// - the code before the bundle writes a name a factory reads inside a
    ///   function, which can run after a module copied the value;
    /// - a read name is one of [`UNBINDABLE_NAMES`];
    /// - the code before the bundle has a module declaration, a `with`
    ///   statement, or a direct `eval`.
    ///
    /// With `after_in_entry`, the code after the bundle stays in entry.js, a
    /// file apart from the prelude, so it must neither redeclare nor write a
    /// name the code before declares, and the code before must not read a
    /// name the code after declares. A chunk's prelude holds both parts.
    pub(super) fn plan(
        before: &[&ModuleItem],
        after: &[&ModuleItem],
        bundle: &[&ModuleItem],
        factory_spans: &[Span],
        after_in_entry: bool,
    ) -> Option<Option<Self>> {
        let mut probe = Module {
            span: DUMMY_SP,
            body: before
                .iter()
                .chain(after)
                .map(|item| (*item).clone())
                .collect(),
            shebang: None,
        };
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        probe.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        let top_level_ctxt = SyntaxContext::empty().apply_mark(top_level_mark);
        let (probe_before, probe_after) = probe.body.split_at(before.len());
        let before_uses = BindingUses::collect(probe_before, top_level_ctxt, None, None);
        let after_uses = BindingUses::collect(probe_after, top_level_ctxt, None, None);
        let declared: HashSet<Atom> = before_uses
            .referenced
            .union(&after_uses.referenced)
            .cloned()
            .collect();

        let factory_uses = factory_uses(&declared, bundle, factory_spans);
        if factory_uses.referenced.is_empty() {
            return Some(None);
        }
        let reads = factory_uses.referenced;
        let read_spans = factory_uses
            .occurrences
            .iter()
            .map(|(_, span)| *span)
            .collect();
        if !factory_uses.written.is_empty()
            || reads.iter().any(|name| {
                after_uses.declared.contains(name)
                    || after_uses.written.contains(name)
                    || before_uses.written_in_function.contains(name)
                    || UNBINDABLE_NAMES.contains(&name.as_ref())
            })
            || before.iter().any(|item| {
                matches!(item, ModuleItem::ModuleDecl(_)) || node_has_dynamic_scope_construct(*item)
            })
        {
            return None;
        }
        if after_in_entry
            && (after_uses
                .declared
                .iter()
                .any(|name| before_uses.declared.contains(name))
                || before_uses
                    .referenced
                    .iter()
                    .any(|name| after_uses.declared.contains(name))
                || after_uses
                    .written
                    .iter()
                    .any(|name| before_uses.declared.contains(name)))
        {
            return None;
        }
        let written =
            |name: &Atom| before_uses.written.contains(name) || after_uses.written.contains(name);
        let name_helpers = find_name_helpers(
            probe_before,
            top_level_ctxt,
            SyntaxContext::empty().apply_mark(unresolved_mark),
        )
        .into_iter()
        .filter(|(helper, alias)| {
            reads.contains(helper)
                && !written(helper)
                && alias.as_ref().is_none_or(|alias| !written(alias))
        })
        .map(|(helper, _)| helper)
        .collect();
        Some(Some(Self {
            name_helpers,
            factory_reads: reads,
            factory_read_spans: read_spans,
            declared: before_uses.declared,
            unstable: before_uses.written_in_function,
            bound: RefCell::default(),
        }))
    }

    /// [`Self::bind`] for a factory module: the candidates are the names a
    /// factory reads. Only those were proven to be the declarations beside
    /// the bundle; a webpack 5 factory inside the bootstrap could read a
    /// bootstrap local of another declared name.
    pub(super) fn bind_factory_reads(
        &self,
        module: &mut Module,
        require_ctxt: SyntaxContext,
        specifier: &str,
        load_anyway: bool,
    ) -> Option<bool> {
        self.bind(
            module,
            &self.factory_reads,
            Some(&self.factory_read_spans),
            require_ctxt,
            specifier,
            load_anyway,
        )
    }

    /// Give `module` a binding for each name of `candidates` it reads as a
    /// free name, as one `var { a, b } = require("<specifier>")` after its
    /// directive prologue; with `load_anyway`, a module that reads none gets
    /// a side-effect `require("<specifier>")` instead. `require_ctxt` is the
    /// context of the module's free `require`. With `read_spans`, only a
    /// free identifier at one of those spans reads the declaration; a name
    /// whose free identifiers all lie elsewhere is not bound, and one with
    /// identifiers on both sides cannot be.
    ///
    /// The inserted declaration binds by name, so it must capture nothing: a
    /// module that may declare one of the read names, or `require`, in its
    /// top-level scope is refused, while a nested declaration of a read name
    /// already shadows it for the code inside. `None` when a read cannot be
    /// bound: the module writes a candidate, reads an unstable name, or has a
    /// `with` statement or a direct `eval` (which could reach a name
    /// dynamically). `Some(true)` when a statement was inserted.
    fn bind(
        &self,
        module: &mut Module,
        candidates: &HashSet<Atom>,
        read_spans: Option<&HashSet<Span>>,
        require_ctxt: SyntaxContext,
        specifier: &str,
        load_anyway: bool,
    ) -> Option<bool> {
        let mut mention = NameMention {
            names: candidates,
            found: false,
        };
        module.visit_with(&mut mention);
        let mut reads: Vec<Atom> = Vec::new();
        if mention.found {
            if has_dynamic_scope_construct(module) {
                return None;
            }
            let mut probe = module.clone();
            let unresolved_mark = resolve_prepared_module(&mut probe);
            let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
            let uses = BindingUses::collect(&probe.body, unresolved_ctxt, Some(candidates), None);
            let mut getters = NamedGetterFinder {
                helpers: &self.name_helpers,
                read_spans,
                unresolved_ctxt,
                calls: HashSet::default(),
                helper_spans: HashSet::default(),
            };
            probe.visit_with(&mut getters);
            if !getters.calls.is_empty() {
                module.visit_mut_with(&mut NamedGetterUnwrapper {
                    calls: &getters.calls,
                });
            }
            let mut own = HashSet::default();
            let mut foreign = HashSet::default();
            for (name, span) in &uses.occurrences {
                if getters.helper_spans.contains(span) {
                    continue;
                }
                if read_spans.is_none_or(|spans| spans.contains(span)) {
                    own.insert(name.clone());
                } else {
                    foreign.insert(name.clone());
                }
            }
            if own.iter().any(|name| {
                foreign.contains(name)
                    || uses.written.contains(name)
                    || self.unstable.contains(name)
            }) {
                return None;
            }
            reads = own.into_iter().collect();
            reads.sort_unstable();
        }
        let declares_require = declares_top_level(module, "require");
        if reads.is_empty() && (!load_anyway || declares_require) {
            return Some(false);
        }
        if declares_require || reads.iter().any(|name| declares_top_level(module, name)) {
            return None;
        }
        let load = prelude_load(&reads, require_ctxt, specifier);
        let insert_at = module
            .body
            .iter()
            .take_while(|item| {
                matches!(item, ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. }))
                    if matches!(&**expr, Expr::Lit(Lit::Str(_))))
            })
            .count();
        module.body.insert(insert_at, load);
        self.bound.borrow_mut().extend(reads);
        Some(true)
    }

    /// The names a module was given a binding for, which the prelude
    /// exports, sorted.
    pub(super) fn exported(&self) -> Vec<Atom> {
        let mut exported: Vec<Atom> = self.bound.borrow().iter().cloned().collect();
        exported.sort_unstable();
        exported
    }
}

/// The prelude file `filename`: `items` in source order, then an export of
/// `exported` when there is any.
pub(super) fn prelude_module(
    items: &[&ModuleItem],
    exported: Vec<Atom>,
    filename: &str,
    cm: &Lrc<SourceMap>,
) -> Option<UnpackedModule> {
    let mut body: Vec<ModuleItem> = items.iter().map(|item| (*item).clone()).collect();
    if !exported.is_empty() {
        body.push(ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(
            NamedExport {
                span: DUMMY_SP,
                specifiers: exported
                    .into_iter()
                    .map(|name| {
                        ExportSpecifier::Named(ExportNamedSpecifier {
                            span: DUMMY_SP,
                            orig: ModuleExportName::Ident(Ident::new_no_ctxt(name, DUMMY_SP)),
                            exported: None,
                            is_type_only: false,
                        })
                    })
                    .collect(),
                src: None,
                type_only: false,
                with: None,
            },
        )));
    }
    let prelude = Module {
        span: DUMMY_SP,
        body,
        shebang: None,
    };
    let (code, generated_source_map) = emit_module_with_source_map(&prelude, cm.clone()).ok()?;
    Some(UnpackedModule {
        id: filename.trim_end_matches(".js").to_string(),
        is_entry: false,
        code,
        filename: filename.to_string(),
        source_ranges: spans_byte_ranges(cm, items.iter().map(|item| item.span())),
        inspection_context_ranges: Vec::new(),
        source_input: String::new(),
        generated_source_map,
        verbatim_source_offset: None,
        mapped_in_every_mode: false,
    })
}

/// esbuild's keep-names helper, declared at the top level of `items`:
/// `var name = (target, value) => defineProperty(target, "name", { value,
/// configurable: true })`, where `defineProperty` is `Object.defineProperty`
/// or a top-level alias of it (`var defineProperty = Object.defineProperty`;
/// producer esbuild@0.25.0 --minify --keep-names). The call sets a
/// function's `name` and returns the function. Returns each helper with the
/// alias it calls, which must not be reassigned either. `top_level_ctxt` and
/// `unresolved_ctxt` are the contexts `items` were resolved with.
fn find_name_helpers(
    items: &[ModuleItem],
    top_level_ctxt: SyntaxContext,
    unresolved_ctxt: SyntaxContext,
) -> Vec<(Atom, Option<Atom>)> {
    let declarators = || {
        items.iter().flat_map(|item| match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => var.decls.as_slice(),
            _ => &[],
        })
    };
    let is_define_property = |expr: &Expr| {
        matches!(expr, Expr::Member(member)
            if matches!(&*member.obj, Expr::Ident(object)
                if object.sym == "Object" && object.ctxt == unresolved_ctxt)
            && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "defineProperty"))
    };
    let aliases: HashSet<Atom> = declarators()
        .filter_map(
            |declarator| match (&declarator.name, declarator.init.as_deref()) {
                (Pat::Ident(binding), Some(init))
                    if binding.ctxt == top_level_ctxt && is_define_property(init) =>
                {
                    Some(binding.sym.clone())
                }
                _ => None,
            },
        )
        .collect();
    declarators()
        .filter_map(|declarator| {
            let Pat::Ident(binding) = &declarator.name else {
                return None;
            };
            let Expr::Arrow(arrow) = declarator.init.as_deref()? else {
                return None;
            };
            if binding.ctxt != top_level_ctxt || arrow.is_async || arrow.is_generator {
                return None;
            }
            let [Pat::Ident(target), Pat::Ident(value)] = arrow.params.as_slice() else {
                return None;
            };
            let ArrowFunctionBody::Expr(body) = &*arrow.body else {
                return None;
            };
            let Expr::Call(call) = strip_parens(body) else {
                return None;
            };
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            let alias = match &**callee {
                Expr::Ident(alias)
                    if alias.ctxt == top_level_ctxt && aliases.contains(&alias.sym) =>
                {
                    Some(alias.sym.clone())
                }
                callee if is_define_property(callee) => None,
                _ => return None,
            };
            let [first, second, third] = call.args.as_slice() else {
                return None;
            };
            if [first, second, third]
                .iter()
                .any(|arg| arg.spread.is_some())
                || !matches!(&*first.expr, Expr::Ident(arg) if arg.to_id() == target.to_id())
                || !matches!(&*second.expr, Expr::Lit(Lit::Str(name)) if name.value == *"name")
                || !is_name_descriptor(&third.expr, &value.id)
            {
                return None;
            }
            Some((binding.sym.clone(), alias))
        })
        .collect()
}

/// `{ value: <value>, configurable: true }`, in either order, with `value`
/// shorthand and `!0` for `true` allowed.
fn is_name_descriptor(expr: &Expr, value: &Ident) -> bool {
    let Expr::Object(object) = expr else {
        return false;
    };
    let mut has_value = false;
    let mut has_configurable = false;
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        match &**prop {
            Prop::Shorthand(ident) if ident.sym == "value" && ident.to_id() == value.to_id() => {
                has_value = true;
            }
            Prop::KeyValue(KeyValueProp {
                key: PropName::Ident(key),
                value: prop_value,
            }) if key.sym == "value" => {
                if !matches!(&**prop_value, Expr::Ident(ident) if ident.to_id() == value.to_id()) {
                    return false;
                }
                has_value = true;
            }
            Prop::KeyValue(KeyValueProp {
                key: PropName::Ident(key),
                value: prop_value,
            }) if key.sym == "configurable" => {
                let is_true = match &**prop_value {
                    Expr::Lit(Lit::Bool(flag)) => flag.value,
                    Expr::Unary(UnaryExpr {
                        op: UnaryOp::Bang,
                        arg,
                        ..
                    }) => matches!(&**arg, Expr::Lit(Lit::Num(num)) if num.value == 0.0),
                    _ => false,
                };
                if !is_true {
                    return false;
                }
                has_configurable = true;
            }
            _ => return false,
        }
    }
    has_value && has_configurable
}

/// A keep-names helper call that wraps a getter webpack's `require.d` installs:
/// `require.d(exports, { key: helper(() => value, "key") })` (producer
/// esbuild@0.25.0 --minify --keep-names over webpack output). The helper
/// returns the getter, and an unwrapped arrow in that property is named
/// `key` too, so the call is the getter itself; `UnEsm` reads only an
/// unwrapped getter. Collects the calls' spans, and the spans of the helper
/// names they read, among the free `helpers` (at `read_spans` when set).
struct NamedGetterFinder<'a> {
    helpers: &'a HashSet<Atom>,
    read_spans: Option<&'a HashSet<Span>>,
    unresolved_ctxt: SyntaxContext,
    calls: HashSet<Span>,
    helper_spans: HashSet<Span>,
}

impl Visit for NamedGetterFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        call.visit_children_with(self);
        if self.helpers.is_empty() {
            return;
        }
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Member(member) = &**callee else {
            return;
        };
        let is_require_d = matches!(&*member.obj, Expr::Ident(require)
                if require.sym == "require" && require.ctxt == self.unresolved_ctxt)
            && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "d");
        let Some(getters) = call.args.get(1) else {
            return;
        };
        let (true, None, Expr::Object(getters)) = (is_require_d, getters.spread, &*getters.expr)
        else {
            return;
        };
        for prop in &getters.props {
            let PropOrSpread::Prop(prop) = prop else {
                continue;
            };
            let Prop::KeyValue(KeyValueProp { key, value }) = &**prop else {
                continue;
            };
            let key = match key {
                PropName::Ident(key) => key.sym.clone(),
                PropName::Str(key) => match key.value.as_str() {
                    Some(key) => Atom::from(key),
                    None => continue,
                },
                _ => continue,
            };
            let Expr::Call(wrapper) = &**value else {
                continue;
            };
            let Callee::Expr(helper) = &wrapper.callee else {
                continue;
            };
            let Expr::Ident(helper) = &**helper else {
                continue;
            };
            let [getter, name] = wrapper.args.as_slice() else {
                continue;
            };
            let is_helper = helper.ctxt == self.unresolved_ctxt
                && self.helpers.contains(&helper.sym)
                && self
                    .read_spans
                    .is_none_or(|spans| spans.contains(&helper.span));
            let is_getter = getter.spread.is_none()
                && matches!(&*getter.expr, Expr::Arrow(arrow) if arrow.params.is_empty());
            let names_key = name.spread.is_none()
                && matches!(&*name.expr, Expr::Lit(Lit::Str(name)) if name.value == *key.as_str());
            if is_helper && is_getter && names_key {
                self.calls.insert(wrapper.span);
                self.helper_spans.insert(helper.span);
            }
        }
    }
}

/// Replaces each call at one of `calls` with its first argument.
struct NamedGetterUnwrapper<'a> {
    calls: &'a HashSet<Span>,
}

impl VisitMut for NamedGetterUnwrapper<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        if let Expr::Call(call) = expr {
            if self.calls.contains(&call.span) {
                if let Some(getter) = call.args.first_mut() {
                    *expr = *std::mem::replace(&mut getter.expr, Expr::undefined(DUMMY_SP));
                }
            }
        }
    }
}

/// `require("<specifier>")` through the free `require` of `require_ctxt`.
fn require_call(require_ctxt: SyntaxContext, specifier: &str) -> Expr {
    Expr::Call(CallExpr {
        callee: Callee::Expr(Box::new(Expr::Ident(Ident::new(
            "require".into(),
            DUMMY_SP,
            require_ctxt,
        )))),
        args: vec![Expr::from(specifier).into()],
        ..Default::default()
    })
}

/// `var { a, b } = require("<specifier>");`, or the side-effect
/// `require("<specifier>");` when `names` is empty.
fn prelude_load(names: &[Atom], require_ctxt: SyntaxContext, specifier: &str) -> ModuleItem {
    let call = Box::new(require_call(require_ctxt, specifier));
    if names.is_empty() {
        return ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: call,
        }));
    }
    let pattern = ObjectPat {
        span: DUMMY_SP,
        props: names
            .iter()
            .map(|name| {
                ObjectPatProp::Assign(AssignPatProp {
                    span: DUMMY_SP,
                    key: BindingIdent::from(Ident::new_no_ctxt(name.clone(), DUMMY_SP)),
                    value: None,
                })
            })
            .collect(),
        optional: false,
        type_ann: None,
    };
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Object(pattern),
            init: Some(call),
            definite: false,
        }],
    }))))
}

/// The uses of `declared` names inside `factory_spans`, which lie in the
/// `bundle` statements, as free names: free in the bundle statements, so in
/// the input they are the top-level declarations beside them. A bundle
/// statement is an expression statement and declares no top-level binding
/// itself. Names are compared first, so the bundle is cloned and resolved
/// only when a factory mentions a declared name at all.
fn factory_uses(
    declared: &HashSet<Atom>,
    bundle: &[&ModuleItem],
    factory_spans: &[Span],
) -> BindingUses {
    if declared.is_empty() || factory_spans.is_empty() {
        return BindingUses::default();
    }
    let mut spans = factory_spans.to_vec();
    spans.sort_unstable_by_key(|span| span.lo);
    let mut mentions = FactoryNameFinder {
        declared,
        spans: &spans,
        found: false,
    };
    for item in bundle {
        item.visit_with(&mut mentions);
    }
    if !mentions.found {
        return BindingUses::default();
    }

    let mut probe = Module {
        span: DUMMY_SP,
        body: bundle.iter().map(|item| (*item).clone()).collect(),
        shebang: None,
    };
    let unresolved_mark = Mark::new();
    probe.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
    BindingUses::collect(
        &probe.body,
        SyntaxContext::empty().apply_mark(unresolved_mark),
        Some(declared),
        Some(&spans),
    )
}

/// Finds an identifier inside one of `spans` (sorted by start) named in
/// `declared`.
struct FactoryNameFinder<'a> {
    declared: &'a HashSet<Atom>,
    spans: &'a [Span],
    found: bool,
}

impl Visit for FactoryNameFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if !self.found && self.declared.contains(&ident.sym) {
            self.found = within(self.spans, ident.span);
        }
    }
}

/// Whether `span` lies inside one of `spans` (sorted by start, disjoint).
fn within(spans: &[Span], span: Span) -> bool {
    let after = spans.partition_point(|outer| outer.lo <= span.lo);
    after > 0 && span.hi <= spans[after - 1].hi
}

/// Whether a node mentions one of `names`.
struct NameMention<'a> {
    names: &'a HashSet<Atom>,
    found: bool,
}

impl Visit for NameMention<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if !self.found && self.names.contains(&ident.sym) {
            self.found = true;
        }
    }
}

/// How code uses the bindings of one syntax context.
#[derive(Default)]
struct BindingUses {
    /// Names declared, by `var`, `let`, `const`, `function`, `class`, or an
    /// import.
    declared: HashSet<Atom>,
    /// Names that appear at all, declarations included.
    referenced: HashSet<Atom>,
    /// Each identifier counted in `referenced`, with its span.
    occurrences: Vec<(Atom, Span)>,
    /// Names assigned, updated, or deleted (an initialized declaration is
    /// not a write).
    written: HashSet<Atom>,
    /// Written names whose write is inside a function or class body.
    written_in_function: HashSet<Atom>,
}

impl BindingUses {
    /// The uses in `items` of bindings with `ctxt`, of `names` when set,
    /// inside `spans` (sorted by start) when set.
    fn collect(
        items: &[ModuleItem],
        ctxt: SyntaxContext,
        names: Option<&HashSet<Atom>>,
        spans: Option<&[Span]>,
    ) -> Self {
        let mut collector = BindingUseCollector {
            ctxt,
            names,
            spans,
            function_depth: 0,
            uses: Self::default(),
        };
        items.visit_with(&mut collector);
        collector.uses
    }
}

struct BindingUseCollector<'a> {
    ctxt: SyntaxContext,
    names: Option<&'a HashSet<Atom>>,
    spans: Option<&'a [Span]>,
    function_depth: usize,
    uses: BindingUses,
}

impl BindingUseCollector<'_> {
    fn counts(&self, ident: &Ident) -> bool {
        ident.ctxt == self.ctxt
            && self.names.is_none_or(|names| names.contains(&ident.sym))
            && self.spans.is_none_or(|spans| within(spans, ident.span))
    }

    fn declare(&mut self, ident: &Ident) {
        if self.counts(ident) {
            self.uses.declared.insert(ident.sym.clone());
        }
    }

    fn write(&mut self, ident: &Ident) {
        if self.counts(ident) {
            self.uses.written.insert(ident.sym.clone());
            if self.function_depth > 0 {
                self.uses.written_in_function.insert(ident.sym.clone());
            }
        }
    }

    fn write_pattern<T>(&mut self, pattern: &T)
    where
        T: VisitWith<swc_core::ecma::utils::DestructuringFinder<Ident>>,
    {
        for ident in find_pat_ids::<_, Ident>(pattern) {
            self.write(&ident);
        }
    }

    fn in_function(&mut self, visit: impl FnOnce(&mut Self)) {
        self.function_depth += 1;
        visit(self);
        self.function_depth -= 1;
    }
}

impl Visit for BindingUseCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if self.counts(ident) {
            self.uses.referenced.insert(ident.sym.clone());
            self.uses.occurrences.push((ident.sym.clone(), ident.span));
        }
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        for ident in find_pat_ids::<_, Ident>(&declarator.name) {
            self.declare(&ident);
        }
        declarator.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.declare(&decl.ident);
        decl.visit_children_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        self.declare(&decl.ident);
        decl.visit_children_with(self);
    }

    fn visit_import_decl(&mut self, decl: &ImportDecl) {
        for ident in find_pat_ids::<_, Ident>(&decl.specifiers) {
            self.declare(&ident);
        }
        decl.visit_children_with(self);
    }

    fn visit_function(&mut self, function: &Function) {
        self.in_function(|this| function.visit_children_with(this));
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.in_function(|this| arrow.visit_children_with(this));
    }

    fn visit_class(&mut self, class: &Class) {
        self.in_function(|this| class.visit_children_with(this));
    }

    fn visit_getter_prop(&mut self, prop: &GetterProp) {
        self.in_function(|this| prop.visit_children_with(this));
    }

    fn visit_setter_prop(&mut self, prop: &SetterProp) {
        self.in_function(|this| prop.visit_children_with(this));
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => self.write(&binding.id),
            AssignTarget::Simple(SimpleAssignTarget::Paren(paren)) => {
                if let Expr::Ident(ident) = strip_parens(&paren.expr) {
                    self.write(ident);
                }
            }
            AssignTarget::Simple(_) => {}
            AssignTarget::Pat(pattern) => self.write_pattern(pattern),
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if let Expr::Ident(ident) = strip_parens(&update.arg) {
            self.write(ident);
        }
        update.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::Delete {
            if let Expr::Ident(ident) = strip_parens(&unary.arg) {
                self.write(ident);
            }
        }
        unary.visit_children_with(self);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        if let ForHead::Pat(pattern) = &stmt.left {
            self.write_pattern(&**pattern);
        }
        stmt.visit_children_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        if let ForHead::Pat(pattern) = &stmt.left {
            self.write_pattern(&**pattern);
        }
        stmt.visit_children_with(self);
    }
}

/// Splits the identifiers of code around the bundle by resolution.
struct SurroundingNameCollector {
    unresolved_ctxt: SyntaxContext,
    top_level_ctxt: SyntaxContext,
    /// Free names and top-level bindings.
    names: HashSet<Atom>,
    top_level_names: HashSet<Atom>,
    all_names: HashSet<Atom>,
}

impl SurroundingNameCollector {
    /// Resolve a copy of `items` as one program and split its identifiers.
    fn collect<'a>(items: impl Iterator<Item = &'a ModuleItem>) -> Self {
        let mut probe = Module {
            span: DUMMY_SP,
            body: items.cloned().collect(),
            shebang: None,
        };
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        probe.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        let mut names = Self {
            unresolved_ctxt: SyntaxContext::empty().apply_mark(unresolved_mark),
            top_level_ctxt: SyntaxContext::empty().apply_mark(top_level_mark),
            names: HashSet::default(),
            top_level_names: HashSet::default(),
            all_names: HashSet::default(),
        };
        probe.visit_with(&mut names);
        names
    }
}

impl Visit for SurroundingNameCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.all_names.insert(ident.sym.clone());
        if ident.ctxt == self.top_level_ctxt {
            self.top_level_names.insert(ident.sym.clone());
            self.names.insert(ident.sym.clone());
        } else if ident.ctxt == self.unresolved_ctxt {
            self.names.insert(ident.sym.clone());
        }
    }
}

/// Symbols of identifiers that carry one syntax context.
pub(super) struct ContextSymCollector {
    ctxt: SyntaxContext,
    pub(super) syms: HashSet<Atom>,
}

impl ContextSymCollector {
    pub(super) fn new(ctxt: SyntaxContext) -> Self {
        Self {
            ctxt,
            syms: HashSet::default(),
        }
    }
}

impl Visit for ContextSymCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.ctxt == self.ctxt {
            self.syms.insert(ident.sym.clone());
        }
    }
}

/// Every identifier name in a node.
#[derive(Default)]
pub(super) struct IdentNameCollector(pub(super) HashSet<Atom>);

impl Visit for IdentNameCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.0.insert(ident.sym.clone());
    }
}
