use std::collections::{HashMap, HashSet};

use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::{
    Decl, ImportDecl, ImportSpecifier, Module, ModuleDecl, ModuleItem, Stmt, Str,
};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::visit::VisitMutWith;

use super::diagnostics::collect_duplicate_declaration_warnings;
use super::io::{parse_js, print_js};
use super::types::{UnpackWarning, UnpackWarningKind};
use super::unpack_cleanup::{
    dedup_duplicate_exports, hoist_late_runtime_helpers, module_export_name_string,
};
use crate::rules::ImportDedup;
use crate::unpacker::module_item_declared_binding_ids;

pub(crate) fn merge_import_cycles(
    modules: Vec<crate::unpacker::UnpackedModule>,
) -> (Vec<crate::unpacker::UnpackedModule>, Vec<UnpackWarning>) {
    const FAST_PREFLIGHT_MIN_MEMBERS: usize = 64;
    const MAX_SAFE_CYCLE_MERGE_MEMBERS: usize = 32;

    let (module_names, graph) = {
        let span = tracing::info_span!("merge_cycles_build_graph");
        let _enter = span.enter();
        let module_pairs: Vec<(String, String)> = modules
            .iter()
            .map(|module| (module.filename.clone(), module.code.clone()))
            .collect();
        let module_names: HashSet<String> = module_pairs
            .iter()
            .map(|(filename, _)| filename.clone())
            .collect();
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        for (filename, code) in &module_pairs {
            graph.insert(
                filename.clone(),
                local_import_dependencies(filename, code, &module_names),
            );
        }
        (module_names, graph)
    };

    let index_by_filename: HashMap<String, usize> = modules
        .iter()
        .enumerate()
        .map(|(index, module)| (module.filename.clone(), index))
        .collect();
    let cycle_components: Vec<Vec<String>> = {
        let span = tracing::info_span!("merge_cycles_tarjan");
        let _enter = span.enter();
        tarjan_sccs(&graph)
            .into_iter()
            .filter(|component| {
                component.len() > 1
                    || component
                        .first()
                        .is_some_and(|filename| graph[filename].contains(filename))
            })
            .collect()
    };
    if cycle_components.is_empty() {
        return (modules, Vec::new());
    }

    let mut file_to_component: HashMap<String, usize> = HashMap::new();
    let mut component_members: Vec<Vec<String>> = Vec::new();
    let mut component_representatives: Vec<String> = Vec::new();
    let mut warnings = Vec::new();
    let module_by_filename: HashMap<String, &crate::unpacker::UnpackedModule> = modules
        .iter()
        .map(|module| (module.filename.clone(), module))
        .collect();

    for mut component in cycle_components {
        component.sort_by_key(|filename| index_by_filename[filename]);
        let representative = component
            .iter()
            .find(|filename| filename.as_str() == "entry.js")
            .cloned()
            .unwrap_or_else(|| component[0].clone());
        let preview = component
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join(" -> ");
        let suffix = if component.len() > 8 { " -> ..." } else { "" };
        let member_set: HashSet<String> = component.iter().cloned().collect();
        if component.len() > MAX_SAFE_CYCLE_MERGE_MEMBERS {
            warnings.push(UnpackWarning::new(
                representative.clone(),
                UnpackWarningKind::ImportCycle,
                format!(
                    "local import cycle across {} modules not merged because it exceeds the large-cycle merge limit of {MAX_SAFE_CYCLE_MERGE_MEMBERS}: {preview}{suffix}",
                    component.len()
                ),
            ));
            continue;
        }
        {
            let span = tracing::info_span!(
                "merge_cycles_preflight_component",
                representative = %representative,
                count = component.len()
            );
            let _enter = span.enter();
            let reason = if should_use_fast_cycle_preflight(&component, FAST_PREFLIGHT_MIN_MEMBERS)
            {
                unsafe_merge_member_reason(
                    &component,
                    &module_by_filename,
                    &module_names,
                    &member_set,
                )
            } else {
                unsafe_merged_cycle_candidate_reason(
                    &representative,
                    &component,
                    &module_by_filename,
                    &module_names,
                    &member_set,
                )
            };
            if let Some(reason) = reason {
                warnings.push(UnpackWarning::new(
                    representative.clone(),
                    UnpackWarningKind::ImportCycle,
                    format!(
                        "local import cycle across {} modules not merged because {reason}: {preview}{suffix}",
                        component.len()
                    ),
                ));
                continue;
            }
        }

        let component_index = component_members.len();
        for filename in &component {
            file_to_component.insert(filename.clone(), component_index);
        }
        component_representatives.push(representative);
        component_members.push(component);
    }
    if component_members.is_empty() {
        return (modules, warnings);
    }

    let file_to_merged: HashMap<String, String> = file_to_component
        .iter()
        .map(|(filename, &component_index)| {
            (
                filename.clone(),
                component_representatives[component_index].clone(),
            )
        })
        .collect();
    let mut emitted_components = HashSet::new();
    let mut merged_modules = Vec::new();

    for module in &modules {
        if let Some(&component_index) = file_to_component.get(&module.filename) {
            if !emitted_components.insert(component_index) {
                continue;
            }
            let representative = &component_representatives[component_index];
            let members = &component_members[component_index];
            let member_set: HashSet<String> = members.iter().cloned().collect();
            let (code, is_entry) = {
                let span = tracing::info_span!(
                    "merge_cycles_emit_component",
                    representative = %representative,
                    count = members.len()
                );
                let _enter = span.enter();
                let (code, is_entry) = build_merged_cycle_code(
                    representative,
                    members,
                    &module_by_filename,
                    &module_names,
                    &file_to_merged,
                    &member_set,
                    true,
                );
                (code, is_entry)
            };
            merged_modules.push(crate::unpacker::UnpackedModule {
                id: representative
                    .strip_suffix(".js")
                    .unwrap_or(representative)
                    .to_string(),
                is_entry,
                code,
                filename: representative.clone(),
            });
        } else {
            let code = if module_imports_retargeted_cycle_member(
                &module.filename,
                &graph,
                &file_to_merged,
            ) {
                rewrite_local_imports_for_merge(
                    &module.filename,
                    &module.filename,
                    &module.code,
                    &module_names,
                    &file_to_merged,
                    &HashSet::new(),
                )
            } else {
                module.code.clone()
            };
            merged_modules.push(crate::unpacker::UnpackedModule {
                id: module.id.clone(),
                is_entry: module.is_entry,
                code,
                filename: module.filename.clone(),
            });
        }
    }

    (merged_modules, warnings)
}

fn module_imports_retargeted_cycle_member(
    filename: &str,
    graph: &HashMap<String, Vec<String>>,
    file_to_merged: &HashMap<String, String>,
) -> bool {
    graph.get(filename).is_some_and(|dependencies| {
        dependencies.iter().any(|dependency| {
            file_to_merged
                .get(dependency)
                .is_some_and(|target| target != dependency)
        })
    })
}

fn should_use_fast_cycle_preflight(component: &[String], min_members: usize) -> bool {
    component.len() >= min_members
        && component
            .iter()
            .any(|filename| !filename.starts_with("module-"))
}

fn build_merged_cycle_code(
    representative: &str,
    members: &[String],
    module_by_filename: &HashMap<String, &crate::unpacker::UnpackedModule>,
    module_names: &HashSet<String>,
    file_to_merged: &HashMap<String, String>,
    member_set: &HashSet<String>,
    dedup_imports: bool,
) -> (String, bool) {
    let is_entry = members
        .iter()
        .any(|member| module_by_filename[member].is_entry);

    let code = GLOBALS
        .set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut imports = Vec::new();
            let mut body = Vec::new();

            for member in members {
                let member_module = module_by_filename[member];
                let mut module =
                    parse_js(&member_module.code, &member_module.filename, cm.clone())?;
                rewrite_local_imports_in_module(
                    &mut module,
                    &member_module.filename,
                    representative,
                    module_names,
                    file_to_merged,
                    member_set,
                );
                for item in module.body {
                    if matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(_))) {
                        imports.push(item);
                    } else {
                        body.push(item);
                    }
                }
            }

            imports.extend(body);
            let mut module = Module {
                span: Default::default(),
                body: imports,
                shebang: None,
            };
            if dedup_imports {
                let unresolved_mark = Mark::new();
                let top_level_mark = Mark::new();
                module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
                module.visit_mut_with(&mut ImportDedup);
                dedup_duplicate_exports(&mut module);
                hoist_late_runtime_helpers(&mut module);
            }
            module.visit_mut_with(&mut fixer(None));
            print_js(&module, cm)
        })
        .unwrap_or_else(|_| {
            let mut code = String::new();
            for member in members {
                let member_module = module_by_filename[member];
                let rewritten = rewrite_local_imports_for_merge(
                    &member_module.filename,
                    representative,
                    &member_module.code,
                    module_names,
                    file_to_merged,
                    member_set,
                );
                if !code.is_empty() {
                    code.push('\n');
                }
                code.push_str(&rewritten);
            }
            code
        });

    (code, is_entry)
}

fn dedup_merged_cycle_imports(filename: &str, code: &str) -> Option<String> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(code, filename, cm.clone()).ok()?;
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        module.visit_mut_with(&mut ImportDedup);
        dedup_duplicate_exports(&mut module);
        hoist_late_runtime_helpers(&mut module);
        module.visit_mut_with(&mut fixer(None));
        print_js(&module, cm).ok()
    })
}

fn unsafe_merged_cycle_candidate_reason(
    representative: &str,
    members: &[String],
    module_by_filename: &HashMap<String, &crate::unpacker::UnpackedModule>,
    module_names: &HashSet<String>,
    member_set: &HashSet<String>,
) -> Option<String> {
    let no_retargets = HashMap::new();
    let (candidate_code, _) = build_merged_cycle_code(
        representative,
        members,
        module_by_filename,
        module_names,
        &no_retargets,
        member_set,
        false,
    );
    let Some(candidate_code) = dedup_merged_cycle_imports(representative, &candidate_code) else {
        return Some("the merged module would not parse".to_string());
    };

    unsafe_merged_cycle_reason(representative, &candidate_code)
}

fn unsafe_merged_cycle_reason(filename: &str, code: &str) -> Option<String> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let Ok(module) = parse_js(code, filename, cm) else {
            return Some("the merged module would not parse".to_string());
        };
        let duplicates = collect_duplicate_declaration_warnings(&module, filename);
        if duplicates.is_empty() {
            return None;
        }

        let preview = duplicates
            .iter()
            .take(5)
            .map(|warning| warning.message.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if duplicates.len() > 5 { ", ..." } else { "" };
        Some(format!(
            "the merged module would create duplicate declarations ({preview}{suffix})"
        ))
    })
}

#[derive(Clone)]
enum MergeBindingOrigin {
    Decl,
    Import(String),
}

pub(crate) fn unsafe_merge_member_reason(
    members: &[String],
    module_by_filename: &HashMap<String, &crate::unpacker::UnpackedModule>,
    module_names: &HashSet<String>,
    member_set: &HashSet<String>,
) -> Option<String> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut seen = HashMap::new();
        let mut duplicates = Vec::new();

        for member in members {
            let member_module = module_by_filename[member];
            let Ok(module) = parse_js(&member_module.code, &member_module.filename, cm.clone())
            else {
                return Some("the merged module would not parse".to_string());
            };

            for item in &module.body {
                if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
                    let Some(specifier) = import.src.value.as_str() else {
                        continue;
                    };
                    if resolve_local_module_specifier(&member_module.filename, specifier)
                        .filter(|resolved| {
                            module_names.contains(resolved) && member_set.contains(resolved)
                        })
                        .is_some()
                    {
                        continue;
                    }

                    for (name, key) in import_binding_keys(import) {
                        record_merge_binding(
                            &mut seen,
                            &mut duplicates,
                            name,
                            MergeBindingOrigin::Import(key),
                        );
                    }
                    continue;
                }

                for name in unsafe_merge_declared_names(item) {
                    record_merge_binding(
                        &mut seen,
                        &mut duplicates,
                        name,
                        MergeBindingOrigin::Decl,
                    );
                }
            }
        }

        if duplicates.is_empty() {
            return None;
        }

        let preview = duplicates
            .iter()
            .take(5)
            .map(|name| format!("duplicate lexical declaration `{name}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if duplicates.len() > 5 { ", ..." } else { "" };
        Some(format!(
            "the merged module would create duplicate declarations ({preview}{suffix})"
        ))
    })
}

fn unsafe_merge_declared_names(item: &ModuleItem) -> Vec<String> {
    let is_var_decl = match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
            var.kind == swc_core::ecma::ast::VarDeclKind::Var
        }
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
            Decl::Var(var) => var.kind == swc_core::ecma::ast::VarDeclKind::Var,
            _ => false,
        },
        _ => false,
    };
    if is_var_decl {
        return Vec::new();
    }

    module_item_declared_binding_ids(item)
        .into_iter()
        .map(|(sym, _)| sym.to_string())
        .collect()
}

fn record_merge_binding(
    seen: &mut HashMap<String, MergeBindingOrigin>,
    duplicates: &mut Vec<String>,
    name: String,
    origin: MergeBindingOrigin,
) {
    if let Some(existing) = seen.get(&name) {
        if let (MergeBindingOrigin::Import(existing), MergeBindingOrigin::Import(next)) =
            (existing, &origin)
        {
            if existing == next {
                return;
            }
        }
        if !duplicates.contains(&name) {
            duplicates.push(name);
        }
        return;
    }

    seen.insert(name, origin);
}

fn import_binding_keys(import: &ImportDecl) -> Vec<(String, String)> {
    let source = import.src.value.as_str().unwrap_or("").to_string();
    import
        .specifiers
        .iter()
        .map(|specifier| match specifier {
            ImportSpecifier::Named(named) => {
                let imported = named
                    .imported
                    .as_ref()
                    .map(module_export_name_string)
                    .unwrap_or_else(|| named.local.sym.to_string());
                (
                    named.local.sym.to_string(),
                    format!("{source}:named:{imported}:{}", named.local.sym),
                )
            }
            ImportSpecifier::Default(default) => (
                default.local.sym.to_string(),
                format!("{source}:default:{}", default.local.sym),
            ),
            ImportSpecifier::Namespace(namespace) => (
                namespace.local.sym.to_string(),
                format!("{source}:namespace:{}", namespace.local.sym),
            ),
        })
        .collect()
}

fn rewrite_local_imports_for_merge(
    source_filename: &str,
    output_filename: &str,
    code: &str,
    module_names: &HashSet<String>,
    file_to_merged: &HashMap<String, String>,
    drop_imports_from: &HashSet<String>,
) -> String {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let Ok(mut module) = parse_js(code, source_filename, cm.clone()) else {
            return code.to_string();
        };

        rewrite_local_imports_in_module(
            &mut module,
            source_filename,
            output_filename,
            module_names,
            file_to_merged,
            drop_imports_from,
        );

        print_js(&module, cm).unwrap_or_else(|_| code.to_string())
    })
}

fn rewrite_local_imports_in_module(
    module: &mut Module,
    source_filename: &str,
    output_filename: &str,
    module_names: &HashSet<String>,
    file_to_merged: &HashMap<String, String>,
    drop_imports_from: &HashSet<String>,
) {
    module.body.retain_mut(|item| {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            return true;
        };
        let Some(specifier) = import.src.value.as_str() else {
            return true;
        };
        let Some(resolved) = resolve_local_module_specifier(source_filename, specifier) else {
            return true;
        };
        if !module_names.contains(&resolved) {
            return true;
        }
        if drop_imports_from.contains(&resolved) {
            return false;
        }

        let target = file_to_merged.get(&resolved).cloned().unwrap_or(resolved);
        let rewritten = relative_import_specifier(output_filename, &target);
        *import.src = Str {
            span: Default::default(),
            value: rewritten.into(),
            raw: None,
        };
        true
    });
}

pub(crate) fn collect_import_cycle_warnings(modules: &[(String, String)]) -> Vec<UnpackWarning> {
    let module_names: HashSet<String> = modules
        .iter()
        .map(|(filename, _)| filename.clone())
        .collect();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    for (filename, code) in modules {
        let deps = local_import_dependencies(filename, code, &module_names);
        graph.insert(filename.clone(), deps);
    }

    tarjan_sccs(&graph)
        .into_iter()
        .filter(|component| {
            component.len() > 1
                || component
                    .first()
                    .is_some_and(|filename| graph[filename].contains(filename))
        })
        .map(|mut component| {
            component.sort();
            let filename = component[0].clone();
            let preview = component
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(" -> ");
            let suffix = if component.len() > 8 { " -> ..." } else { "" };
            UnpackWarning::new(
                filename,
                UnpackWarningKind::ImportCycle,
                format!(
                    "local import cycle across {} modules: {preview}{suffix}",
                    component.len()
                ),
            )
        })
        .collect()
}

fn local_import_dependencies(
    filename: &str,
    code: &str,
    module_names: &HashSet<String>,
) -> Vec<String> {
    if !filename.starts_with("module-") {
        if let Some(deps) = scan_local_import_dependencies(filename, code, module_names) {
            return deps;
        }
    }

    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let Ok(module) = parse_js(code, filename, cm) else {
            return vec![];
        };
        let mut deps: Vec<String> = module
            .body
            .iter()
            .filter_map(|item| {
                let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                    return None;
                };
                import
                    .src
                    .value
                    .as_str()
                    .and_then(|specifier| resolve_local_module_specifier(filename, specifier))
            })
            .filter(|dep| module_names.contains(dep))
            .collect();
        deps.sort();
        deps.dedup();
        deps
    })
}

pub(crate) fn scan_local_import_dependencies(
    filename: &str,
    code: &str,
    module_names: &HashSet<String>,
) -> Option<Vec<String>> {
    let mut deps = Vec::new();
    let mut statement = String::new();
    let mut in_import = false;

    for line in code.lines() {
        let trimmed = line.trim_start();
        if !in_import {
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            if line != trimmed || !is_static_import_start(trimmed) {
                continue;
            }
            statement.clear();
            statement.push_str(trimmed);
            in_import = !trimmed.contains(';');
        } else {
            statement.push(' ');
            statement.push_str(trimmed);
            in_import = !trimmed.contains(';');
        }

        if !in_import {
            let specifier = extract_static_import_specifier(&statement)?;
            if let Some(dep) = resolve_local_module_specifier(filename, specifier)
                .filter(|d| module_names.contains(d))
            {
                deps.push(dep);
            }
        }
    }

    if in_import {
        return None;
    }

    deps.sort();
    deps.dedup();
    Some(deps)
}

fn is_static_import_start(line: &str) -> bool {
    line == "import" || line.starts_with("import ") || line.starts_with("import{")
}

fn extract_static_import_specifier(statement: &str) -> Option<&str> {
    let import_tail = statement.strip_prefix("import")?.trim_start();
    let specifier_start = if import_tail.starts_with('"') || import_tail.starts_with('\'') {
        import_tail
    } else {
        let from_index = statement.rfind(" from ")?;
        statement[from_index + " from ".len()..].trim_start()
    };
    let mut chars = specifier_start.char_indices();
    let (_, quote) = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let start = quote.len_utf8();
    for (index, ch) in chars {
        if ch == quote {
            return Some(&specifier_start[start..index]);
        }
    }
    None
}

fn resolve_local_module_specifier(filename: &str, specifier: &str) -> Option<String> {
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        return None;
    }

    let mut parts: Vec<&str> = filename.split('/').collect();
    parts.pop();
    for part in specifier.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

fn relative_import_specifier(from_filename: &str, to_filename: &str) -> String {
    let from_dir: Vec<&str> = from_filename
        .rsplit_once('/')
        .map(|(dir, _)| dir.split('/').collect())
        .unwrap_or_default();
    let to_parts: Vec<&str> = to_filename.split('/').collect();
    let to_file = *to_parts.last().unwrap_or(&to_filename);
    let to_dir = &to_parts[..to_parts.len().saturating_sub(1)];

    let mut common = 0;
    while common < from_dir.len() && common < to_dir.len() && from_dir[common] == to_dir[common] {
        common += 1;
    }

    let mut parts = Vec::new();
    for _ in common..from_dir.len() {
        parts.push("..".to_string());
    }
    for part in &to_dir[common..] {
        parts.push((*part).to_string());
    }
    parts.push(to_file.to_string());

    let specifier = parts.join("/");
    if specifier.starts_with('.') {
        specifier
    } else {
        format!("./{specifier}")
    }
}

fn tarjan_sccs(graph: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    struct Tarjan<'a> {
        graph: &'a HashMap<String, Vec<String>>,
        index: usize,
        stack: Vec<String>,
        on_stack: HashSet<String>,
        indices: HashMap<String, usize>,
        lowlinks: HashMap<String, usize>,
        components: Vec<Vec<String>>,
    }

    impl Tarjan<'_> {
        fn strong_connect(&mut self, node: String) {
            self.indices.insert(node.clone(), self.index);
            self.lowlinks.insert(node.clone(), self.index);
            self.index += 1;
            self.stack.push(node.clone());
            self.on_stack.insert(node.clone());

            for dep in self.graph.get(&node).into_iter().flatten() {
                if !self.indices.contains_key(dep) {
                    self.strong_connect(dep.clone());
                    let low = self.lowlinks[&node].min(self.lowlinks[dep]);
                    self.lowlinks.insert(node.clone(), low);
                } else if self.on_stack.contains(dep) {
                    let low = self.lowlinks[&node].min(self.indices[dep]);
                    self.lowlinks.insert(node.clone(), low);
                }
            }

            if self.lowlinks[&node] == self.indices[&node] {
                let mut component = Vec::new();
                while let Some(member) = self.stack.pop() {
                    self.on_stack.remove(&member);
                    let done = member == node;
                    component.push(member);
                    if done {
                        break;
                    }
                }
                self.components.push(component);
            }
        }
    }

    let mut tarjan = Tarjan {
        graph,
        index: 0,
        stack: Vec::new(),
        on_stack: HashSet::new(),
        indices: HashMap::new(),
        lowlinks: HashMap::new(),
        components: Vec::new(),
    };
    let mut nodes: Vec<String> = graph.keys().cloned().collect();
    nodes.sort();
    for node in nodes {
        if !tarjan.indices.contains_key(&node) {
            tarjan.strong_connect(node);
        }
    }
    tarjan.components
}
