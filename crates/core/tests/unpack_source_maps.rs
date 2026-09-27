//! Unpack output source maps point into the unpacked input.

use std::fs;

use wakaru_core::driver::test_support::unpack;
use wakaru_core::{DecompileOptions, RewriteLevel};

fn fixture(path: &str) -> String {
    let full = format!("tests/bundles/{path}");
    fs::read_to_string(&full).unwrap_or_else(|e| panic!("failed to read {full}: {e}"))
}

fn mapped_options(filename: &str, level: RewriteLevel, heuristic_split: bool) -> DecompileOptions {
    DecompileOptions {
        filename: filename.to_string(),
        level,
        heuristic_split,
        emit_source_map: true,
        ..Default::default()
    }
}

/// The token starting at `col` (UTF-16 units) of `line`.
fn token_at(text: &str, line: u32, col: u32) -> String {
    let Some(line) = text.split('\n').nth(line as usize) else {
        return String::new();
    };
    let units: Vec<u16> = line.encode_utf16().collect();
    let rest = String::from_utf16_lossy(&units[(col as usize).min(units.len())..]);
    let mut chars = rest.chars();
    match chars.next() {
        Some(quote @ ('"' | '\'')) => {
            let body: String = chars.take_while(|&ch| ch != quote).collect();
            format!("{quote}{body}{quote}")
        }
        Some(first) if first.is_alphanumeric() || first == '_' || first == '$' => {
            std::iter::once(first)
                .chain(chars.take_while(|&ch| ch.is_alphanumeric() || ch == '_' || ch == '$'))
                .collect()
        }
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

struct MapCheck {
    maps: usize,
    mapped_tokens: usize,
    same_tokens: usize,
}

/// Every map names `input_name`, embeds no source text, and maps each
/// string literal to the same literal in `input`. Rules rewrite other
/// tokens (`var` → `const`, `!0` → `true`, renames), so those are only
/// counted. Import specifiers are rewritten to output filenames.
fn check_maps_point_into_input(
    input: &str,
    input_name: &str,
    modules: &[(String, String)],
    source_maps: &[(String, String)],
) -> MapCheck {
    let mut check = MapCheck {
        maps: 0,
        mapped_tokens: 0,
        same_tokens: 0,
    };
    for (filename, map_json) in source_maps {
        let map = sourcemap::SourceMap::from_reader(map_json.as_bytes())
            .unwrap_or_else(|e| panic!("{filename}: invalid map: {e}"));
        let code = &modules
            .iter()
            .find(|(name, _)| name == filename)
            .unwrap_or_else(|| panic!("{filename}: map without module"))
            .1;
        check.maps += 1;
        if map.get_token_count() == 0 {
            continue;
        }
        assert_eq!(
            map.sources().collect::<Vec<_>>(),
            vec![input_name],
            "{filename}: map must name the input"
        );
        assert_eq!(map.get_source_contents(0), None, "{filename}");
        let mut positions: Vec<_> = map
            .tokens()
            .map(|token| (token.get_dst_line(), token.get_dst_col()))
            .collect();
        let total = positions.len();
        positions.dedup();
        assert_eq!(
            positions.len(),
            total,
            "{filename}: one mapping per output position"
        );
        for token in map.tokens() {
            check.mapped_tokens += 1;
            let output = token_at(code, token.get_dst_line(), token.get_dst_col());
            let original = token_at(input, token.get_src_line(), token.get_src_col());
            if output == original {
                check.same_tokens += 1;
            }
            let is_literal = output.len() >= 2 && output.starts_with(['"', '\'']);
            if !is_literal || output[1..].starts_with("./") || output[1..].starts_with("../") {
                continue;
            }
            assert_eq!(
                output[1..output.len() - 1],
                original[1.min(original.len())..original.len().saturating_sub(1)],
                "{filename}: literal at {}:{} maps to {original:?} at {}:{}",
                token.get_dst_line(),
                token.get_dst_col(),
                token.get_src_line(),
                token.get_src_col()
            );
        }
    }
    check
}

fn assert_fixture_maps(path: &str, level: RewriteLevel, heuristic_split: bool) {
    let source = fixture(path);
    let plain = unpack(
        &source,
        DecompileOptions {
            emit_source_map: false,
            ..mapped_options(path, level, heuristic_split)
        },
    )
    .unwrap_or_else(|e| panic!("{path}: {e}"));
    let mapped = unpack(&source, mapped_options(path, level, heuristic_split))
        .unwrap_or_else(|e| panic!("{path}: {e}"));

    // Requesting maps must not change what is unpacked.
    assert_eq!(plain.modules, mapped.modules, "{path}: code changed");
    assert_eq!(
        format!("{:?}", plain.provenance),
        format!("{:?}", mapped.provenance),
        "{path}: provenance changed"
    );

    let check = check_maps_point_into_input(&source, path, &mapped.modules, &mapped.source_maps);
    assert!(check.maps > 0, "{path}: no maps");
    assert!(check.mapped_tokens > 0, "{path}: nothing mapped");
    assert!(
        check.same_tokens * 2 >= check.mapped_tokens,
        "{path}: only {} of {} mapped tokens match the input",
        check.same_tokens,
        check.mapped_tokens
    );
}

#[test]
fn webpack_maps_point_into_the_bundle() {
    for path in [
        "webpack-gen/dist/wp4-cjs/bundle.js",
        "webpack-gen/dist/wp4-esm/bundle.js",
        "webpack-gen/dist/wp5-cjs-min/bundle.js",
        "webpack-gen/dist/wp5-array/bundle.js",
        "webpack-gen/dist/wp5-amd-return-min/bundle.js",
    ] {
        assert_fixture_maps(path, RewriteLevel::Standard, false);
    }
}

#[test]
fn prepared_module_formats_map_into_the_bundle() {
    for path in [
        "cocos-creator-gen/dist/project.js",
        "metro-gen/dist/min.bundle.js",
        "closure-module-manager-gen/dist/compiler-chunks/bundle.js",
    ] {
        assert_fixture_maps(path, RewriteLevel::Standard, false);
    }
}

#[test]
fn esbuild_and_bun_maps_point_into_the_bundle() {
    for path in [
        "esbuild-gen/dist/es-mixed/bundle.js",
        "esbuild-gen/dist/iife-factories/bundle.js",
        "esbuild-gen/dist/bun-cross-ref-min/bundle.js",
        "bun-gen/dist/es/entry.js",
    ] {
        assert_fixture_maps(path, RewriteLevel::Standard, false);
    }
}

#[test]
fn heuristic_scope_hoist_maps_point_into_the_bundle() {
    for path in [
        "rollup-gen/dist/es/bundle.mjs",
        "vite-gen/dist/es-min/bundle.mjs",
    ] {
        assert_fixture_maps(path, RewriteLevel::Standard, true);
    }
}

#[test]
fn systemjs_maps_point_into_the_bundle() {
    for path in [
        "systemjs-gen/dist/babel/entry.js",
        "systemjs-gen/dist/webpack-system/bundle.js",
    ] {
        assert_fixture_maps(path, RewriteLevel::Standard, false);
    }
}

#[test]
fn nested_scope_split_children_map_through_their_parent() {
    let path = "bun-gen/dist/cjs-interop/entry-cjs.js";
    assert_fixture_maps(path, RewriteLevel::Aggressive, true);

    let source = fixture(path);
    let mapped = unpack(
        &source,
        mapped_options(path, RewriteLevel::Aggressive, true),
    )
    .expect("unpack should succeed");
    let children: Vec<_> = mapped
        .source_maps
        .iter()
        .filter(|(filename, _)| filename.contains('/'))
        .collect();
    assert!(
        !children.is_empty(),
        "the fixture must split a nested child"
    );
    for (filename, map_json) in children {
        let map = sourcemap::SourceMap::from_reader(map_json.as_bytes()).unwrap();
        assert!(map.get_token_count() > 0, "{filename}: child map is empty");
        assert_eq!(map.sources().collect::<Vec<_>>(), vec![path], "{filename}");
    }
}

#[test]
fn verbatim_system_register_maps_by_offset() {
    // The first register keeps an object `_export` in expression position,
    // so it is emitted as a verbatim copy of its `System.register` call.
    let source = "System.register(\"a\", [], function (e) {\n  return { execute: function () { use(e({ a: \"first\" })); } };\n});\nSystem.register(\"b\", [], function (e) {\n  return { execute: function () { e(\"b\", \"second\"); } };\n});\n";
    let output = unpack(
        source,
        mapped_options("registers.js", RewriteLevel::Standard, false),
    )
    .expect("unpack should succeed");
    let (verbatim, _) = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("System.register"))
        .expect("the unlowerable register should stay verbatim");
    let check =
        check_maps_point_into_input(source, "registers.js", &output.modules, &output.source_maps);
    assert!(check.same_tokens > 0);
    let map = &output
        .source_maps
        .iter()
        .find(|(filename, _)| filename == verbatim)
        .expect("verbatim module should have a map")
        .1;
    let map = sourcemap::SourceMap::from_reader(map.as_bytes()).unwrap();
    assert!(map.get_token_count() > 0);
}

#[test]
fn unpack_map_columns_count_utf16_units() {
    let source = "(()=>{var e={12:(e,t,r)=>{var s=\"中文字\",n=foo(s);e.exports=n}},t={};function r(n){var o=t[n];if(void 0!==o)return o.exports;var i=t[n]={exports:{}};return e[n](i,i.exports,r),i.exports}r(12)})();\n";
    let output = unpack(
        source,
        mapped_options("wide.js", RewriteLevel::Standard, false),
    )
    .expect("unpack should succeed");
    let (filename, code) = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("foo(s)"))
        .expect("the factory should be recovered");
    let map = &output
        .source_maps
        .iter()
        .find(|(name, _)| name == filename)
        .unwrap()
        .1;
    let map = sourcemap::SourceMap::from_reader(map.as_bytes()).unwrap();
    let line = code
        .lines()
        .position(|line| line.contains("foo(s)"))
        .unwrap() as u32;
    let col = code
        .lines()
        .nth(line as usize)
        .unwrap()
        .find("foo")
        .unwrap() as u32;
    let token = map
        .lookup_token(line, col)
        .expect("the call should be mapped");
    let expected = source[..source.find("foo").unwrap()].encode_utf16().count() as u32;
    assert_eq!((token.get_src_line(), token.get_src_col()), (0, expected));
}

#[test]
fn unpack_maps_count_positions_after_a_byte_order_mark() {
    let body = "(()=>{var e={12:(e,t,r)=>{e.exports=\"after-bom\"}},t={};function r(n){var o=t[n];if(void 0!==o)return o.exports;var i=t[n]={exports:{}};return e[n](i,i.exports,r),i.exports}r(12)})();\n";
    let source = format!("\u{feff}{body}");
    let output = unpack(
        &source,
        mapped_options("bom.js", RewriteLevel::Standard, false),
    )
    .expect("unpack should succeed");
    // Positions are measured in the decoded script, which has no BOM.
    let check = check_maps_point_into_input(body, "bom.js", &output.modules, &output.source_maps);
    assert!(check.same_tokens > 0);
}
