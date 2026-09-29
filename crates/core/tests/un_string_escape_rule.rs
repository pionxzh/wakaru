mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnStringEscape;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnStringEscape)
}

#[test]
fn decodes_printable_non_ascii_escapes() {
    let input = r#"
const a = "\u4e2d\u6587";
const b = 'You\u2019ve';
const c = "caf\xe9";
const d = "\u{1F600}";
const e = "\uD83D\uDE00";
const f = { "\u540d\u524d": 1 };
"#;
    let expected = r#"
const a = "中文";
const b = 'You’ve';
const c = "café";
const d = "😀";
const e = "😀";
const f = { "名前": 1 };
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_other_escapes_and_quote_style() {
    // Only the non-ASCII escape is decoded; ASCII escapes, quotes and
    // backslashes keep their original spelling.
    let input = r#"
const a = '\u4e2d \'x\' \n \\u4e2d \x41 \u0041';
"#;
    let expected = r#"
const a = '中 \'x\' \n \\u4e2d \x41 \u0041';
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_escapes_for_invisible_or_unsafe_characters() {
    // Lone surrogates, controls, separators, whitespace, format and bidi
    // controls, BOM, variation selectors and private-use characters stay
    // escaped: they either cannot be printed or are invisible in source.
    let input = r#"
const a = "\uD800 \uDC00 \x85 \u2028 \u2029 \xa0 \u3000 \u200b \u200d \u202e \u2066 \ufeff \ufe0f \ue000 \xad \u{E0041}";
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_surrogate_pairs_that_are_regex_range_endpoints() {
    // `\uDB7F\uDC00` here is the end of one range and the start of the next,
    // not the single character they happen to form.
    let input = r#"
const a = "\uD800-\uDB7F\uDC00-\uDFFF";
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_template_literal_raw_text() {
    // A template's raw text is observable through `String.raw` and tag
    // functions, so it is never rewritten.
    let input = r#"
const a = `\u4e2d`;
const b = String.raw`\u4e2d`;
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_jsx_attribute_strings() {
    // JSX attribute strings do not process escapes: `"\u4e2d"` there is a
    // literal backslash followed by `u4e2d`.
    let input = r#"
const a = <div title="\u4e2d" />;
"#;
    assert_eq_normalized(&apply(input), input);
}
