use swc_core::common::{Span, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_core::ecma::ast::Decl;

use crate::unpacker::emit_esm::emit_module_raw;

use super::bindings::{
    add_factory_atom_import, atom_to_filename_binding_map, collect_top_level_decl_indices,
    collect_top_level_decl_references, collect_top_level_decl_writes, AtomRefCollector,
};
use super::emit::{repair_module_imports, restore_demoted_factories};
use super::factories::{
    collect_commonjs_helper_syms, collect_factories, collect_factory_analysis_bindings,
    collect_helper_syms, filter_helper_factory_declarators, has_factory_detection_evidence,
};
use super::ownership::{
    augment_imports_with_referenced_atoms_for_existing_sources, canonical_factory_filename,
    claim_standalone_ownership, place_top_level_writers, plan_demotion, plan_merged_module,
    union_writer_groups, FactoryOwnership, MergedFactory, MergedPlanContext, PendingFactory,
    SupportClaimFilter, TopLevelIndex, TopLevelWriterItem,
};
use super::synthesis::{
    emit_items, factory_owned_export_names, filter_item_to_owned_bindings, relative_import_path,
    retain_owned_support_source_items, scope_owned_support_decl_items,
};
use super::*;

fn assert_owned_factory_detector_matches_borrowed(
    source: &str,
    filename: &str,
    expect_commonjs_evidence: bool,
) {
    let cm: Lrc<SourceMap> = Default::default();
    let module =
        super::super::parse_es_module(source, filename, cm.clone()).expect("fixture should parse");
    let helper_syms = collect_helper_syms(&module);
    assert_eq!(
        !collect_commonjs_helper_syms(&module).is_empty(),
        expect_commonjs_evidence,
        "fixture must exercise the intended evidence path"
    );
    assert!(
        has_factory_detection_evidence(&module, &helper_syms),
        "fixture must pass the owned detector's preflight"
    );

    let borrowed = detect_from_module_with_source(
        &module,
        Some(source),
        cm.clone(),
        crate::unpacker::SourcePositions::Discard,
    )
    .expect("borrowed detector should accept fixture");
    let owned = detect_from_owned_factory_module_with_source(
        module,
        Some(source),
        cm,
        crate::unpacker::SourcePositions::Discard,
    )
    .expect("owned detector should accept every preflight-approved fixture");
    let module_pairs = |result: UnpackResult| {
        result
            .modules
            .into_iter()
            .map(|module| (module.filename, module.code))
            .collect::<Vec<_>>()
    };
    assert_eq!(module_pairs(owned), module_pairs(borrowed));
}

#[test]
fn owned_destructuring_keeps_sibling_dependencies_and_declaration_metadata() {
    GLOBALS.set(&Default::default(), || {
        let mut module = super::super::parse_es_module(
            "let discarded = function () { return unrelated; }, dependency = value, \
             { left = dependency, right } = source, trailing = other;",
            "owned-destructuring.js",
            Default::default(),
        )
        .unwrap();
        module.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(original))) = &module.body[0] else {
            panic!("expected variable declaration");
        };
        let filtered = filter_item_to_owned_bindings(
            &module.body[0],
            &HashSet::from_iter([Atom::from("left")]),
        )
        .unwrap();
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(filtered))) = filtered else {
            panic!("expected filtered variable declaration");
        };
        assert_eq!(filtered.span, original.span);
        assert_eq!(filtered.ctxt, original.ctxt);
        assert_eq!(filtered.kind, original.kind);
        assert_eq!(filtered.declare, original.declare);
        assert_eq!(filtered.decls, original.decls[1..3]);
        assert_eq!(original.decls.len(), 4);
    });
}

#[test]
fn declaration_metadata_is_independent_of_worker_count() {
    GLOBALS.set(&Default::default(), || {
        let mut source = String::from("var state = 0, shared = 1, shadowed = 2; ");
        for index in 0..128 {
            source.push_str(&format!(
                "var value{index} = shared, writer{index} = function (shadowed) {{ \
                 state += value{index}; return shadowed; }}; "
            ));
        }
        let mut module =
            super::super::parse_es_module(&source, "parallel-metadata.js", Default::default())
                .unwrap();
        module.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));
        let bindings = module
            .body
            .iter()
            .flat_map(module_item_declared_binding_ids)
            .collect::<HashSet<_>>();
        let indices = collect_top_level_decl_indices(&module.body);
        let collect = |threads| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            GLOBALS.with(|globals| {
                pool.install(|| {
                    GLOBALS.set(globals, || {
                        (
                            collect_top_level_decl_references(
                                &module.body,
                                &indices,
                                &bindings,
                                &HashSet::default(),
                            ),
                            collect_top_level_decl_writes(&module.body, &indices, &bindings),
                        )
                    })
                })
            })
        };
        let single = collect(1);
        assert_eq!(single, collect(4));
        let binding = |name: &str| bindings.iter().find(|id| id.0 == name).unwrap();
        let writer = binding("writer0");
        assert!(single.0[writer].contains(binding("value0")));
        assert!(!single.0[writer].contains(binding("shadowed")));
        assert!(single.1[writer].contains(binding("state")));
    });
}

#[test]
fn top_level_declaration_index_reuses_items_with_multiple_bindings() {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = super::super::parse_es_module(
            "var first = 1, second = 2; function third() {}",
            "decl-indices.js",
            cm,
        )
        .expect("fixture should parse");
        module.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));

        let indices = collect_top_level_decl_indices(&module.body);
        let by_name = indices
            .into_iter()
            .map(|((name, _), index)| (name.to_string(), index))
            .collect::<HashMap<_, _>>();

        assert_eq!(by_name.get("first"), Some(&0));
        assert_eq!(by_name.get("second"), Some(&0));
        assert_eq!(by_name.get("third"), Some(&1));
    });
}

#[test]
fn top_level_declaration_references_stay_binding_specific() {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = super::super::parse_es_module(
            "var first_source = 1, second_source = 2; \
             var first = first_source, second = second_source;",
            "decl-references.js",
            cm,
        )
        .expect("fixture should parse");
        module.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));

        let top_level_bindings = module
            .body
            .iter()
            .flat_map(module_item_declared_binding_ids)
            .collect::<HashSet<_>>();
        let indices = collect_top_level_decl_indices(&module.body);
        let references = collect_top_level_decl_references(
            &module.body,
            &indices,
            &top_level_bindings,
            &HashSet::default(),
        );
        let binding_named = |name: &str| {
            top_level_bindings
                .iter()
                .find(|binding| binding.0 == *name)
                .cloned()
                .unwrap()
        };

        let first_references = &references[&binding_named("first")];
        assert!(first_references.contains(&binding_named("first_source")));
        assert!(!first_references.contains(&binding_named("second_source")));

        let second_references = &references[&binding_named("second")];
        assert!(second_references.contains(&binding_named("second_source")));
        assert!(!second_references.contains(&binding_named("first_source")));
    });
}

#[test]
fn support_item_retention_keeps_only_claimed_original_items() {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = super::super::parse_es_module(
            "var keep = 1, owned = 2; var untouched = 3;",
            "support-items.js",
            cm,
        )
        .expect("fixture should parse");
        module.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));

        let owned_binding = module_item_declared_binding_ids(&module.body[0])
            .into_iter()
            .find(|(name, _)| name == "owned")
            .expect("owned binding should exist");
        let owned = HashSet::from_iter([owned_binding.clone()]);
        let owned_by_index = HashMap::from_iter([(0, owned.clone())]);
        let mut source_slots = module.body.into_iter().map(Some).collect::<Vec<_>>();

        let originals = retain_owned_support_source_items(&mut source_slots, &owned_by_index);

        assert_eq!(originals.len(), 1);
        assert!(originals.contains_key(&0));
        assert!(!originals.contains_key(&1));
        let remaining_names = module_item_declared_binding_ids(
            source_slots[0]
                .as_ref()
                .expect("unclaimed sibling should remain"),
        )
        .into_iter()
        .map(|(name, _)| name.to_string())
        .collect::<HashSet<_>>();
        assert_eq!(remaining_names, HashSet::from_iter(["keep".to_string()]));

        let recovered = scope_owned_support_decl_items(
            &owned,
            &HashMap::from_iter([(owned_binding, 0)]),
            &originals,
        );
        let recovered_names = recovered
            .iter()
            .flat_map(module_item_declared_binding_ids)
            .map(|(name, _)| name.to_string())
            .collect::<HashSet<_>>();
        assert_eq!(recovered_names, HashSet::from_iter(["owned".to_string()]));
    });
}

#[test]
fn factory_analysis_bindings_are_read_from_the_resolved_ast_by_location() {
    GLOBALS.set(&Default::default(), || {
        let source = r#"
var wrap = (q, K) => () => (K || q((K = { exports: {} }).exports, K), K.exports);
var shared = 1, assigned = 0;
var ignored = 1, value = wrap(() => assigned = shared);
"#;
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::super::parse_es_module(source, "factory-location.js", cm).unwrap();
        let mut analysis_module = module.clone();
        analysis_module.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));

        let helper_syms = collect_helper_syms(&module);
        let commonjs_helper_syms = collect_commonjs_helper_syms(&module);
        let factories = collect_factories(
            &module,
            &analysis_module,
            &helper_syms,
            &commonjs_helper_syms,
            None,
        );
        let factory = factories
            .iter()
            .find(|factory| factory.var_name == *"value")
            .expect("factory in the second declarator should be collected");
        let top_level_bindings = analysis_module
            .body
            .iter()
            .flat_map(module_item_declared_binding_ids)
            .collect::<HashSet<_>>();
        let binding_named = |name: &str| {
            top_level_bindings
                .iter()
                .find(|binding| binding.0 == *name)
                .cloned()
                .unwrap()
        };

        let (references, writes) = collect_factory_analysis_bindings(
            &analysis_module,
            factory.analysis_location,
            &top_level_bindings,
        )
        .expect("resolved factory body should be found by location");

        assert!(references.contains(&binding_named("shared")));
        assert!(references.contains(&binding_named("assigned")));
        assert_eq!(writes, HashSet::from_iter([binding_named("assigned")]));
    });
}

#[test]
fn owned_factory_detector_moves_bodies_without_changing_output() {
    GLOBALS.set(&Default::default(), || {
        let source = r#"
var wrap = (q, K) => () => (K || q((K = { exports: {} }).exports, K), K.exports);
var value = wrap((exports, module) => { module.exports = 42; });
console.log(value());
"#;
        assert_owned_factory_detector_matches_borrowed(source, "owned-factory.js", true);

        let rejected_source = "var y = (q, K) => () => q; var only = y(() => 1);";
        let rejected_cm: Lrc<SourceMap> = Default::default();
        let rejected_module = super::super::parse_es_module(
            rejected_source,
            "rejected-owned-factory.js",
            rejected_cm.clone(),
        )
        .unwrap();
        let before = emit_module_raw(&rejected_module, rejected_cm.clone()).unwrap();
        let rejected = match detect_from_owned_factory_module_with_source(
            rejected_module,
            Some(rejected_source),
            rejected_cm.clone(),
            crate::unpacker::SourcePositions::Discard,
        ) {
            Ok(_) => panic!("one non-CommonJS factory is insufficient evidence"),
            Err(rejected) => rejected,
        };
        let after = emit_module_raw(&rejected, rejected_cm).unwrap();
        assert_eq!(after, before, "rejected candidates must not be mutated");
    });
}

#[test]
fn owned_factory_detector_preflight_covers_every_movable_body_shape() {
    GLOBALS.set(&Default::default(), || {
        let fixtures = [
            (
                "arrow-block",
                r#"
var y = (q, K) => () => (q && (K = q(q = 0)), K);
var one = y(() => { first(); });
var two = y(() => { second(); });
var three = y(() => { third(); });
var four = y(() => { fourth(); });
var five = y(() => { fifth(); });
"#,
            ),
            (
                "arrow-expression",
                r#"
var y = (q, K) => () => (q && (K = q(q = 0)), K);
var one = y(() => first());
var two = y(() => second());
var three = y(() => third());
var four = y(() => fourth());
var five = y(() => fifth());
"#,
            ),
            (
                "function-expression",
                r#"
var y = (q, K) => () => (q && (K = q(q = 0)), K);
var one = y(function() { first(); });
var two = y(function() { second(); });
var three = y(function() { third(); });
var four = y(function() { fourth(); });
var five = y(function() { fifth(); });
"#,
            ),
            (
                "object-method",
                r#"
var y = (q, K) => () => (q && (K = q(q = 0)), K);
var one = y({ "one.js"() { first(); } });
var two = y({ "two.js"() { second(); } });
var three = y({ "three.js"() { third(); } });
var four = y({ "four.js"() { fourth(); } });
var five = y({ "five.js"() { fifth(); } });
"#,
            ),
        ];

        for (shape, source) in fixtures {
            assert_owned_factory_detector_matches_borrowed(
                source,
                &format!("owned-{shape}.js"),
                false,
            );
        }
    });
}

#[test]
fn entry_filter_drops_factory_only_declarations_and_keeps_mixed_siblings() {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::super::parse_es_module(
            "var factory = () => 1; var other_factory = () => 2, keep = 3;",
            "entry-filter.js",
            cm,
        )
        .expect("fixture should parse");
        let factory_syms = HashSet::from_iter([Atom::from("factory"), Atom::from("other_factory")]);

        assert!(filter_helper_factory_declarators(&module.body[0], &factory_syms).is_none());
        let filtered = filter_helper_factory_declarators(&module.body[1], &factory_syms)
            .expect("mixed declaration should keep its non-factory sibling");
        let remaining = module_item_declared_binding_ids(&filtered)
            .into_iter()
            .map(|(atom, _)| atom)
            .collect::<HashSet<_>>();
        assert_eq!(remaining, HashSet::from_iter([Atom::from("keep")]));
    });
}

fn collect_atom_refs(source: &str, candidates: &[&str]) -> HashSet<Atom> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::super::parse_es_module(source, "atom-ref-test.js", cm)
            .expect("test source should parse");
        let candidate_atoms: HashSet<Atom> =
            candidates.iter().map(|name| Atom::from(*name)).collect();
        let mut collector = AtomRefCollector {
            candidate_atoms: &candidate_atoms,
            references: HashSet::default(),
            shadowed_atoms: vec![HashSet::default()],
        };
        module.visit_with(&mut collector);
        collector.references
    })
}

#[test]
fn atom_ref_collector_finds_unbound_candidate_refs() {
    let refs = collect_atom_refs(
        r#"
function read(q) {
    return JA(q);
}
var obj = { [JA]: true };
"#,
        &["JA"],
    );

    assert!(refs.contains(&Atom::from("JA")));
}

#[test]
fn atom_ref_collector_skips_shadowed_refs_and_static_property_keys() {
    let refs = collect_atom_refs(
        r#"
function read(JA) {
    return JA;
}
var obj = { JA: true };
"#,
        &["JA"],
    );

    assert!(
        !refs.contains(&Atom::from("JA")),
        "parameter references and static object keys should not synthesize imports"
    );
}

#[test]
fn atom_ref_collector_treats_assignment_targets_as_references() {
    let refs = collect_atom_refs(
        r#"
JA = value;
({ answer: KA = fallback } = source);
use(JA, KA);
"#,
        &["JA", "KA"],
    );

    assert_eq!(
        refs,
        HashSet::from_iter([Atom::from("JA"), Atom::from("KA")]),
        "assignment targets are uses of existing bindings, not shadow declarations"
    );
}

#[test]
fn import_repair_keeps_assigned_relocated_bindings_visible() {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module =
            super::super::parse_es_module("state = next; observe(state);", "entry.js", cm.clone())
                .expect("fixture should parse");
        let binding_to_filename = HashMap::from_iter([(
            (Atom::from("state"), Default::default()),
            "owner.js".to_string(),
        )]);

        let repaired = repair_module_imports(module.body, "entry.js", &binding_to_filename);
        let output = emit_items(
            repaired,
            "entry.js".to_string(),
            cm,
            SourcePositions::Discard,
        )
        .code;

        assert!(
            output.contains("import { state } from \"./owner.js\""),
            "the relocated binding write must not suppress import repair:\n{output}"
        );
    });
}

#[test]
fn import_augmentation_only_adds_specifiers_to_existing_sources() {
    let mut binding_to_filename = HashMap::default();
    binding_to_filename.insert((Atom::from("NT"), Default::default()), "NT.js".to_string());
    binding_to_filename.insert((Atom::from("JA"), Default::default()), "NT.js".to_string());
    binding_to_filename.insert(
        (Atom::from("Other"), Default::default()),
        "Other.js".to_string(),
    );
    let referenced_atoms = [Atom::from("JA"), Atom::from("Other")]
        .into_iter()
        .collect();
    let mut imports_by_source =
        HashMap::from_iter([(String::from("NT.js"), vec![Atom::from("NT")])]);
    let binding_filename_by_atom = atom_to_filename_binding_map(&binding_to_filename);

    augment_imports_with_referenced_atoms_for_existing_sources(
        &mut imports_by_source,
        "D38_2.js",
        &referenced_atoms,
        &binding_filename_by_atom,
        None,
    );

    let nt_imports = imports_by_source.get("NT.js").unwrap();
    assert!(nt_imports.contains(&Atom::from("JA")));
    assert!(
        !imports_by_source.contains_key("Other.js"),
        "augmentation must not create new import edges"
    );
}

#[test]
fn factory_atom_import_can_create_filename_edge() {
    let mut imports_by_filename = HashMap::default();
    let binding = (Atom::from("RT6"), Default::default());

    add_factory_atom_import(&mut imports_by_filename, "Zaq_2.js", &binding, "RT6.js");

    assert_eq!(
        imports_by_filename.get("RT6.js"),
        Some(&vec![binding.clone()])
    );

    add_factory_atom_import(&mut imports_by_filename, "RT6.js", &binding, "RT6.js");

    assert_eq!(
        imports_by_filename.get("RT6.js"),
        Some(&vec![binding]),
        "self imports should still be ignored"
    );
}

fn test_binding(name: &str) -> BindingId {
    (Atom::from(name), SyntaxContext::empty())
}

fn test_bindings(names: &[&str]) -> HashSet<BindingId> {
    names.iter().map(|name| test_binding(name)).collect()
}

fn pending_factory(name: &str, referenced: &[&str], writes: &[&str]) -> PendingFactory {
    PendingFactory {
        binding: test_binding(name),
        var_name: Atom::from(name),
        filename: format!("{name}.js"),
        cjs_params: None,
        body_stmts: Vec::new(),
        referenced_bindings: test_bindings(referenced),
        write_bindings: test_bindings(writes),
        span: DUMMY_SP,
    }
}

#[test]
fn writer_groups_use_the_first_member_as_canonical_and_mark_only_writer_groups() {
    let factories = vec![
        // Writes state owned by a later factory: the group still takes
        // the earlier member's file as canonical.
        pending_factory("a", &[], &["c_state"]),
        // Owns a support declaration whose body writes `b_state`.
        pending_factory("b", &[], &[]),
        pending_factory("c", &[], &[]),
        // Writes only its own state: no redirect, but still a writer.
        pending_factory("d", &[], &["d_state"]),
        pending_factory("e", &[], &[]),
        pending_factory("f", &[], &[]),
    ];
    let binding_to_filename: HashMap<BindingId, String> = [
        ("c_state", "c.js"),
        ("f_state", "f.js"),
        ("d_state", "d.js"),
    ]
    .into_iter()
    .map(|(binding, filename)| (test_binding(binding), filename.to_string()))
    .collect();
    let factory_owned_bindings: HashMap<String, HashSet<BindingId>> =
        [("b.js".to_string(), test_bindings(&["b_support"]))]
            .into_iter()
            .collect();
    let top_level_decl_writes: HashMap<BindingId, HashSet<BindingId>> =
        [(test_binding("b_support"), test_bindings(&["f_state"]))]
            .into_iter()
            .collect();

    let groups = union_writer_groups(
        &factories,
        &binding_to_filename,
        &factory_owned_bindings,
        &top_level_decl_writes,
    );

    let mut redirects: Vec<(&str, &str)> = groups
        .redirects
        .iter()
        .map(|(member, canonical)| (member.as_str(), canonical.as_str()))
        .collect();
    redirects.sort_unstable();
    assert_eq!(redirects, [("c.js", "a.js"), ("f.js", "b.js")]);
    let mut affected: Vec<&str> = groups.affected.iter().map(String::as_str).collect();
    affected.sort_unstable();
    assert_eq!(affected, ["a.js", "b.js", "d.js"]);
    assert_eq!(
        canonical_factory_filename(&groups.redirects, "f.js"),
        "b.js"
    );
    assert_eq!(
        canonical_factory_filename(&groups.redirects, "e.js"),
        "e.js"
    );
}

#[test]
fn standalone_ownership_claims_direct_references_before_expanding_closures() {
    // `a` reaches `shared` only through its support declaration, while
    // `b` references it directly; the direct claim wins even though `a`
    // comes first.
    let factories = vec![
        pending_factory(
            "a",
            &["a_support", "runtime", "global", "taken"],
            &["a_state"],
        ),
        pending_factory("b", &["shared"], &[]),
    ];
    let helper_syms: HashSet<Atom> = [Atom::from("runtime")].into_iter().collect();
    let factory_syms: HashSet<Atom> = [Atom::from("a"), Atom::from("b")].into_iter().collect();
    let top_level_decl_indices: HashMap<BindingId, usize> = [
        "a_state",
        "a_support",
        "shared",
        "shared_dep",
        "runtime",
        "taken",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, name)| (test_binding(name), index))
    .collect();
    let top_level_decl_references: HashMap<BindingId, HashSet<BindingId>> = [
        (test_binding("a_support"), test_bindings(&["shared"])),
        (test_binding("shared"), test_bindings(&["shared_dep"])),
    ]
    .into_iter()
    .collect();
    let filter = SupportClaimFilter {
        helper_syms: &helper_syms,
        factory_syms: &factory_syms,
        top_level_decl_indices: &top_level_decl_indices,
    };
    let mut binding_to_filename: HashMap<BindingId, String> =
        [(test_binding("taken"), "scope.js".to_string())]
            .into_iter()
            .collect();
    let mut factory_owned_bindings = HashMap::default();

    claim_standalone_ownership(
        &factories,
        &top_level_decl_references,
        &filter,
        &mut binding_to_filename,
        &mut factory_owned_bindings,
    );

    let owner = |name: &str| {
        binding_to_filename
            .get(&test_binding(name))
            .map(String::as_str)
    };
    assert_eq!(owner("a"), Some("a.js"));
    assert_eq!(owner("a_state"), Some("a.js"));
    assert_eq!(owner("a_support"), Some("a.js"));
    assert_eq!(owner("shared"), Some("b.js"));
    assert_eq!(owner("shared_dep"), Some("b.js"));
    assert_eq!(owner("taken"), Some("scope.js"));
    assert_eq!(owner("runtime"), None);
    assert_eq!(owner("global"), None);
    assert_eq!(
        factory_owned_bindings.get("a.js"),
        Some(&test_bindings(&["a_state", "a_support"]))
    );
    assert_eq!(
        factory_owned_bindings.get("b.js"),
        Some(&test_bindings(&["shared", "shared_dep"]))
    );
}

#[test]
fn merged_module_plans_publish_adopted_declarations_to_later_modules() {
    let merged = |name: &str, referenced: &[&str]| MergedFactory {
        var_name: Atom::from(name),
        cjs_params: None,
        stmts: Vec::new(),
        referenced_bindings: test_bindings(referenced),
        write_bindings: HashSet::default(),
    };
    let index = TopLevelIndex {
        decl_indices: [(test_binding("support"), 3)].into_iter().collect(),
        decl_binding_by_atom: HashMap::default(),
        decl_references: HashMap::default(),
        decl_writes: HashMap::default(),
        external_imports: HashMap::default(),
    };
    let empty_external_atoms = HashMap::default();
    let empty_imports = HashMap::default();
    let empty_local_atoms = HashMap::default();
    let empty_referenced = HashMap::default();
    let empty_owners = HashMap::default();
    let context = MergedPlanContext {
        index: &index,
        external_import_by_atom: &empty_external_atoms,
        module_already_imports: &empty_imports,
        module_local_atoms: &empty_local_atoms,
        module_referenced_atoms: &empty_referenced,
        pre_merge_binding_filename_by_atom: &empty_owners,
    };
    let mut ownership = FactoryOwnership::new(HashMap::default());

    let first = plan_merged_module(
        0,
        "first.js",
        vec![merged("init_first", &["support"])],
        &context,
        &mut ownership,
    );
    let second = plan_merged_module(
        1,
        "second.js",
        vec![merged("init_second", &["support"])],
        &context,
        &mut ownership,
    );

    assert_eq!(
        first.owned_items,
        vec![(3, [Atom::from("support")].into_iter().collect())]
    );
    assert!(first.named_imports.is_empty());
    assert_eq!(first.export_names, vec![Atom::from("support")]);
    assert_eq!(
        first.export_names,
        factory_owned_export_names("first.js", &ownership.factory_owned_bindings),
        "a later module's adoption must not change an earlier plan's exports"
    );
    assert!(second.owned_items.is_empty());
    assert!(second.export_names.is_empty());
    assert_eq!(
        second.named_imports,
        vec![(
            relative_import_path("second.js", "first.js"),
            vec![Atom::from("support")]
        )]
    );
    assert_eq!(
        ownership
            .binding_to_filename
            .get(&test_binding("support"))
            .map(String::as_str),
        Some("first.js")
    );
    assert_eq!(
        ownership.entry_duplicate_declarations,
        test_bindings(&["support"])
    );
    assert!(first
        .helper_reserved_atoms
        .contains(&Atom::from("init_first")));
}

fn empty_top_level_index() -> TopLevelIndex {
    TopLevelIndex {
        decl_indices: HashMap::default(),
        decl_binding_by_atom: HashMap::default(),
        decl_references: HashMap::default(),
        decl_writes: HashMap::default(),
        external_imports: HashMap::default(),
    }
}

fn writer_item(
    source_index: usize,
    writes: &[&str],
    declared: &[&str],
    relocatable_shape: bool,
) -> TopLevelWriterItem {
    TopLevelWriterItem {
        source_index,
        write_targets: test_bindings(writes),
        referenced_bindings: HashSet::default(),
        declared_bindings: test_bindings(declared),
        relocatable_shape,
        span: DUMMY_SP,
    }
}

#[test]
fn top_level_writers_relocate_join_or_request_demotion() {
    GLOBALS.set(&Default::default(), || {
        let module = super::super::parse_es_module(
            "state = 1; function bump() { state++; } var copy = state = 2;",
            "writers.js",
            Default::default(),
        )
        .expect("fixture should parse");
        let factories = vec![pending_factory("group", &[], &[])];
        let mut ownership = FactoryOwnership::new(HashMap::default());
        ownership.own(test_binding("state"), "group.js");
        let index = empty_top_level_index();
        let remaining_entry_spans: HashSet<(u32, u32)> =
            [(DUMMY_SP.lo.0, DUMMY_SP.hi.0)].into_iter().collect();

        let demotions = place_top_level_writers(
            vec![
                // A plain statement moves with the state it writes.
                writer_item(0, &["state"], &[], true),
                // A stable function declaration joins the group.
                writer_item(1, &["state"], &["bump"], false),
                // A declaration of an entry binding cannot move.
                writer_item(2, &["state"], &["copy"], false),
            ],
            &module.body,
            &remaining_entry_spans,
            &factories,
            &index,
            &mut ownership,
        );

        assert_eq!(
            demotions,
            ["group.js".to_string()].into_iter().collect::<HashSet<_>>()
        );
        let relocated: Vec<usize> = ownership.relocated_writers["group.js"]
            .iter()
            .map(|writer| writer.source_index)
            .collect();
        assert_eq!(relocated, [0]);
        assert_eq!(
            ownership.factory_owned_bindings["group.js"],
            test_bindings(&["state", "bump"])
        );
        assert!(ownership.affected.contains("group.js"));
    });
}

#[test]
fn demotion_cascades_to_dependent_groups_and_refuses_merged_dependents() {
    let factories = vec![
        pending_factory("provider", &[], &[]),
        pending_factory("consumer", &["provided"], &[]),
        pending_factory("unrelated", &[], &[]),
    ];
    let mut ownership = FactoryOwnership::new(HashMap::default());
    ownership.own(test_binding("provided"), "provider.js");
    let index = empty_top_level_index();
    let requested: HashSet<String> = ["provider.js".to_string()].into_iter().collect();
    let no_merged = HashMap::default();
    let no_scope_refs = HashMap::default();
    let no_entry_spans = HashSet::default();

    let demoted = plan_demotion(
        requested.clone(),
        &factories,
        &no_merged,
        &no_scope_refs,
        &no_entry_spans,
        &index,
        &ownership,
    )
    .expect("only standalone groups depend on the demoted binding");
    let mut demoted: Vec<&str> = demoted.iter().map(String::as_str).collect();
    demoted.sort_unstable();
    assert_eq!(demoted, ["consumer.js", "provider.js"]);

    let merged_dependent: HashMap<String, Vec<MergedFactory>> = [(
        "scope.js".to_string(),
        vec![MergedFactory {
            var_name: Atom::from("init_scope"),
            cjs_params: None,
            stmts: Vec::new(),
            referenced_bindings: test_bindings(&["provided"]),
            write_bindings: HashSet::default(),
        }],
    )]
    .into_iter()
    .collect();
    assert!(plan_demotion(
        requested,
        &factories,
        &merged_dependent,
        &no_scope_refs,
        &no_entry_spans,
        &index,
        &ownership,
    )
    .is_none());
}

#[test]
fn ownership_reports_bindings_listed_under_a_file_that_does_not_own_them() {
    let mut ownership = FactoryOwnership::new(HashMap::default());
    ownership.own(test_binding("state"), "first.js");
    ownership.own(test_binding("scope_state"), "scope.js");
    assert!(ownership.ownership_conflicts().is_empty());

    // A second standalone writer lists the same state: grouping should have
    // joined the two files.
    ownership
        .factory_owned_bindings
        .entry("second.js".to_string())
        .or_default()
        .insert(test_binding("state"));
    assert_eq!(
        ownership.ownership_conflicts(),
        vec![(test_binding("state"), "second.js".to_string())]
    );
}

#[test]
fn demoted_factories_return_to_their_source_position_with_fresh_guards() {
    GLOBALS.set(&Default::default(), || {
        let module = super::super::parse_es_module(
            "var before = 1; var __wakaru_init_initialized = 0; var after = 2;",
            "demoted.js",
            Default::default(),
        )
        .expect("fixture should parse");
        let mut remaining_entry = module.body.clone();
        let mut factory = pending_factory("init", &[], &[]);
        factory.span = Span::new(module.body[1].span().hi, module.body[2].span().lo);

        restore_demoted_factories(&mut remaining_entry, vec![factory]);

        let declared: Vec<Vec<String>> = remaining_entry
            .iter()
            .map(|item| {
                module_item_declared_binding_ids(item)
                    .into_iter()
                    .map(|(atom, _)| atom.to_string())
                    .collect()
            })
            .collect();
        let first_restored = 2;
        let last_restored = declared.len() - 2;
        assert_eq!(
            declared[..first_restored],
            [vec!["before"], vec!["__wakaru_init_initialized"]]
        );
        assert_eq!(declared[declared.len() - 1], ["after"]);
        let restored: Vec<&String> = declared[first_restored..=last_restored]
            .iter()
            .flatten()
            .collect();
        assert!(restored.iter().any(|name| name.as_str() == "init"));
        assert!(restored
            .iter()
            .any(|name| name.starts_with("__wakaru_init_initialized")
                && name.as_str() != "__wakaru_init_initialized"));
    });
}

#[test]
fn affected_group_files_include_redirected_members_only_of_affected_groups() {
    let mut ownership = FactoryOwnership::new(HashMap::default());
    ownership.affected.insert("writer.js".to_string());
    ownership
        .redirects
        .insert("writer_member.js".to_string(), "writer.js".to_string());
    ownership
        .redirects
        .insert("plain_member.js".to_string(), "plain.js".to_string());

    let mut files: Vec<&str> = ownership
        .affected_group_files()
        .map(String::as_str)
        .collect();
    files.sort_unstable();
    assert_eq!(files, ["writer.js", "writer_member.js"]);
}
