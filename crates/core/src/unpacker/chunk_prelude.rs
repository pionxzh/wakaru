//! Top-level code beside a webpack chunk's push: a raw banner or footer, an
//! injected snippet such as a release or debug-id record, or a concatenated
//! script. It runs when the chunk script loads and belongs to no module, so
//! it goes to a prelude file of its own, and every module of the chunk gets a
//! side-effect `require` of that file, or one that binds the prelude's
//! declarations the module reads. The code is kept and still runs once,
//! before any of the chunk's modules evaluates; a prelude file nothing
//! required would never run.

use swc_core::common::{sync::Lrc, SourceMap, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_core::ecma::ast::{
    ArrowExpr, CallExpr, Callee, ClassDecl, Expr, ExprStmt, FnDecl, Function, GetterProp, Ident,
    Lit, Module, ModuleItem, SetterProp, Stmt, VarDeclarator,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::module_path::relative_import_specifier;
use crate::unpacker::surrounding::{prelude_module, PreludeNames};
use crate::unpacker::{PreparedModuleAst, UnpackedModule};

/// Whether a top-level item beside the chunk containers is worth a prelude.
/// Directives and empty statements alone are not, as for the code around a
/// bootstrap.
pub(super) fn is_prelude_code(item: &ModuleItem) -> bool {
    match item {
        ModuleItem::Stmt(Stmt::Empty(_)) => false,
        ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) => {
            !matches!(&**expr, Expr::Lit(Lit::Str(_)))
        }
        _ => true,
    }
}

/// The prelude file's name: `prelude-<id>.js` after the chunk's first numeric
/// id, otherwise `prelude.js`.
pub(super) fn prelude_filename(chunk_id: Option<usize>) -> String {
    match chunk_id {
        Some(id) => format!("prelude-{id}.js"),
        None => "prelude.js".to_string(),
    }
}

/// Write `items` (in source order) to the prelude file `filename` and give
/// each prepared chunk module a side-effect `require` of it. With `names`,
/// a module that reads a name the prelude declares gets
/// `var { name } = require(...)` instead, and the prelude exports the name
/// (see [`PreludeNames`]). Returns `None` when no module can carry the
/// `require` (every factory is opaque or empty, or declares its own
/// top-level `require`), a read cannot be bound, or the name is taken: the
/// code would then run from no output file, or a module would read an
/// unbound name, so the caller keeps the input whole.
pub(super) fn attach(
    items: &[&ModuleItem],
    filename: &str,
    names: Option<&PreludeNames>,
    modules: &mut Vec<UnpackedModule>,
    prepared: &mut Vec<Option<PreparedModuleAst>>,
    cm: &Lrc<SourceMap>,
) -> Option<()> {
    if modules
        .iter()
        .any(|module| module.filename.eq_ignore_ascii_case(filename))
    {
        return None;
    }
    let mut required = false;
    for (module, prepared) in modules.iter().zip(prepared.iter_mut()) {
        let Some(prepared) = prepared else {
            continue;
        };
        // An empty factory's body is what tells the driver to restore
        // webpack's implicit `module.exports = {}`; it runs nothing that the
        // prelude must precede.
        if prepared.module.body.is_empty() {
            continue;
        }
        let specifier = relative_import_specifier(&module.filename, filename);
        if let Some(names) = names {
            let PreparedModuleAst {
                globals,
                module,
                unresolved_mark,
                ..
            } = prepared;
            required |= GLOBALS.set(globals, || {
                let require_ctxt = SyntaxContext::empty().apply_mark(*unresolved_mark);
                names.bind_factory_reads(module, require_ctxt, &specifier, true)
            })?;
            continue;
        }
        if declares_top_level(&prepared.module, "require") {
            continue;
        }
        insert_require(prepared, &specifier);
        required = true;
    }
    if !required {
        return None;
    }

    let exported = names.map(PreludeNames::exported).unwrap_or_default();
    let prelude = prelude_module(items, exported, filename, cm)?;
    modules.push(prelude);
    prepared.push(None);
    Some(())
}

/// `require("<specifier>");` after the module's directive prologue. The free
/// `require` is the module's own; `UnEsm` turns the call into a side-effect
/// import when the module becomes ESM and keeps it when it stays CommonJS.
fn insert_require(prepared: &mut PreparedModuleAst, specifier: &str) {
    let unresolved_ctxt = GLOBALS.set(&prepared.globals, || {
        SyntaxContext::empty().apply_mark(prepared.unresolved_mark)
    });
    let call = ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Call(CallExpr {
            callee: Callee::Expr(Box::new(Expr::Ident(Ident::new(
                "require".into(),
                DUMMY_SP,
                unresolved_ctxt,
            )))),
            args: vec![Expr::from(specifier).into()],
            ..Default::default()
        })),
    }));
    let body = &mut prepared.module.body;
    let insert_at = body
        .iter()
        .take_while(|item| {
            matches!(item, ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. }))
                if matches!(&**expr, Expr::Lit(Lit::Str(_))))
        })
        .count();
    body.insert(insert_at, call);
}

/// Whether `name` may be declared in the module's top-level scope: by a
/// top-level declaration, or by a `var` or function declaration in a nested
/// block that hoists to it. A block-scoped declaration also counts, which
/// errs toward not inserting a call it could capture.
pub(super) fn declares_top_level(module: &Module, name: &str) -> bool {
    let mut finder = TopLevelDeclarationFinder { name, found: false };
    module.visit_with(&mut finder);
    finder.found
}

struct TopLevelDeclarationFinder<'a> {
    name: &'a str,
    found: bool,
}

impl Visit for TopLevelDeclarationFinder<'_> {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        let ids: Vec<Ident> = find_pat_ids(&declarator.name);
        if ids.iter().any(|id| id.sym == self.name) {
            self.found = true;
        }
        declarator.init.visit_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        if decl.ident.sym == self.name {
            self.found = true;
        }
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        if decl.ident.sym == self.name {
            self.found = true;
        }
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_class(&mut self, _: &swc_core::ecma::ast::Class) {}

    fn visit_getter_prop(&mut self, _: &GetterProp) {}

    fn visit_setter_prop(&mut self, _: &SetterProp) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declares(source: &str) -> bool {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let module = super::super::parse_es_module(source, "input.js", cm).unwrap();
            declares_top_level(&module, "require")
        })
    }

    #[test]
    fn top_level_and_hoisted_declarations_count() {
        assert!(declares("var require = 1;"));
        assert!(declares("if (a) { var require = 1; }"));
        assert!(declares("function require() {}"));
        assert!(declares("for (var [require] of a);"));
    }

    #[test]
    fn declarations_inside_functions_do_not_count() {
        assert!(!declares("function f(require) { var require = 1; }"));
        assert!(!declares(
            "a(() => { var require; }); var o = { get x() { var require; } };"
        ));
        assert!(!declares("class C { m() { var require; } }"));
        assert!(!declares("require('./a.js');"));
    }
}
