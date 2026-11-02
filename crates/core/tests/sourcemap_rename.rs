mod common;

use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::transforms::base::fixer::fixer;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_core::sourcemap_rename::apply_sourcemap_renames;

fn rename_with_map(source: &str, map_json: &str) -> String {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = common::parse_module_with_filename(source, "bundle.js", cm.clone());
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        let sm = wakaru_core::parse_sourcemap(map_json.as_bytes()).expect("source map parses");
        apply_sourcemap_renames(&mut module, &sm, &cm, unresolved_mark);
        module.visit_mut_with(&mut fixer(None));
        common::emit_module(&module, cm)
    })
}

#[test]
fn identifier_inside_a_computed_object_key_votes_for_its_original_name() {
    // The only mapped token is the `a` inside `[a]` on the second line
    // (generated column 11), carrying the original name `key`.
    let source = "var a = 1;\nvar o = { [a]: 2 };\n";
    let map = r#"{
        "version": 3,
        "sources": ["orig.js"],
        "names": ["key"],
        "mappings": ";WACAA"
    }"#;
    let output = rename_with_map(source, map);
    assert!(output.contains("var key = 1;"), "{output}");
    assert!(output.contains("[key]: 2"), "{output}");
}

#[test]
fn source_paths_drop_traversal_written_with_either_separator() {
    let out_dir = std::path::Path::new("out");
    for (source, expected) in [
        ("webpack:///../src/main.js", "src/main.js"),
        ("webpack-internal:///./src/a.js", "src/a.js"),
        ("webpack://my-lib/./src/b.js", "my-lib/src/b.js"),
        // A drive-like segment mid-path stays a segment, its colon replaced;
        // one at the front is a drive.
        ("src/D:/evil.js", "src/D_/evil.js"),
        ("webpack:///C:/proj/src/a.js", "proj/src/a.js"),
        ("/C:/proj/src/b.js", "proj/src/b.js"),
        ("../x:y.js", "y.js"),
        (r"..\..\evil.js", "evil.js"),
        (r"src\..\..\evil.js", "src/evil.js"),
        (r"\\server\share\evil.js", "server/share/evil.js"),
        (r"..\..\", ""),
    ] {
        let resolved = wakaru_core::resolve_source_path(out_dir, source);
        let relative = resolved.strip_prefix(out_dir).expect("stays under out_dir");
        let parts: Vec<_> = relative
            .components()
            .map(|component| match component {
                std::path::Component::Normal(part) => part.to_str().expect("utf-8").to_string(),
                other => panic!("{source:?} resolved to a non-normal component {other:?}"),
            })
            .collect();
        assert_eq!(parts.join("/"), expected, "{source:?}");
    }
}
