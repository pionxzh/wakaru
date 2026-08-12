use std::collections::{HashMap, HashSet, VecDeque};

use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};
use swc_core::ecma::ast::{ModuleDecl, ModuleItem};

use super::io::parse_js;
use super::types::{UnpackWarning, UnpackWarningKind};

// Import-cycle handling is diagnostics-only. A driver-level "premerge" that
// concatenated local import SCCs before Phase 1 was built alongside these
// warnings and deliberately never enabled; see
// docs/learnings/import-cycle-premerge.md for why it was removed.
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
            let witness = deterministic_cycle_witness(&graph, &component).join(" -> ");
            let filename = component[0].clone();
            UnpackWarning::new(
                filename,
                UnpackWarningKind::ImportCycle,
                format!(
                    "local import cycle across {} modules; cycle witness: {witness}",
                    component.len()
                ),
            )
        })
        .collect()
}

/// Return a deterministic closed path whose adjacent filenames are all real
/// graph edges. SCC membership alone does not provide such an order: sorting
/// member names and joining them with arrows can invent nonexistent edges.
fn deterministic_cycle_witness(
    graph: &HashMap<String, Vec<String>>,
    component: &[String],
) -> Vec<String> {
    let start = component
        .first()
        .expect("cycle components are non-empty")
        .clone();
    let members: HashSet<&str> = component.iter().map(String::as_str).collect();

    if component.len() == 1 {
        debug_assert!(graph[&start].contains(&start));
        return vec![start.clone(), start];
    }

    // Find the shortest path from the lexicographically first member back to
    // itself. Sorted neighbors make ties deterministic regardless of HashMap
    // insertion order. Skip a self-edge here so a multi-member SCC reports a
    // witness that demonstrates at least one of its cross-module edges.
    let mut queue = VecDeque::new();
    let mut predecessor: HashMap<String, String> = HashMap::new();
    let mut start_deps = graph[&start]
        .iter()
        .filter(|dep| dep.as_str() != start && members.contains(dep.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    start_deps.sort();
    start_deps.dedup();
    for dep in start_deps {
        predecessor.insert(dep.clone(), start.clone());
        queue.push_back(dep);
    }

    while let Some(node) = queue.pop_front() {
        let mut deps = graph[&node]
            .iter()
            .filter(|dep| members.contains(dep.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        deps.sort();
        deps.dedup();

        if deps.contains(&start) {
            let mut reversed = vec![node.clone()];
            let mut cursor = node;
            while cursor != start {
                cursor = predecessor[&cursor].clone();
                reversed.push(cursor.clone());
            }
            reversed.reverse();
            reversed.push(start);
            return reversed;
        }

        for dep in deps {
            if dep != start && !predecessor.contains_key(&dep) {
                predecessor.insert(dep.clone(), node.clone());
                queue.push_back(dep);
            }
        }
    }

    unreachable!("every nontrivial SCC contains a cycle through each member")
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
