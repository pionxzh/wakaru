mod common;

use common::{assert_eq_normalized, render_pipeline};

fn apply(input: &str) -> String {
    render_pipeline(input)
}

#[test]
fn iife_single_char_params_renamed_to_longer_ident_args() {
    let input = r#"
(function(i, s, o, g, r, a, m) {
  i['GoogleAnalyticsObject'] = r;
  i[r] = i[r] || function() { (i[r].q = i[r].q||[]).push(arguments) }
  i[r].l = 1 * new Date();
  a = s.createElement(o);
  m = s.getElementsByTagName(o)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
})(window, document, 'script', 'https://www.google-analytics.com/analytics.js', 'ga');
"#;
    // ArrowFunction rule converts the IIFE's function expression to an arrow.
    // The inner function using `arguments` is preserved as a function expression.
    let expected = r#"
((window, document, o, g, r, a, m) => {
  window['GoogleAnalyticsObject'] = r;
  window[r] = window[r] || function() { (window[r].q = window[r].q||[]).push(arguments) }
  window[r].l = 1 * new Date();
  a = document.createElement(o);
  m = document.getElementsByTagName(o)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
})(window, document, 'script', 'https://www.google-analytics.com/analytics.js', 'ga');
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_literal_args_extracted_to_const_when_no_arguments_usage() {
    let input = r#"
!function(i, s, o, g, r, a, m) {
  i['GoogleAnalyticsObject'] = r;
  i[r].l = 1 * new Date();
  a = s.createElement(o);
  m = s.getElementsByTagName(o)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
}(window, document, 'script', 'https://www.google-analytics.com/analytics.js', 'ga');
"#;
    // ArrowFunction rule converts the IIFE's function expression to an arrow.
    let expected = r#"
!((window, document, a, m) => {
  const o = 'script';
  const g = 'https://www.google-analytics.com/analytics.js';
  const r = 'ga';
  window['GoogleAnalyticsObject'] = r;
  window[r].l = 1 * new Date();
  a = document.createElement(o);
  m = document.getElementsByTagName(o)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
})(window, document);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_param_with_longer_name_not_renamed() {
    let input = r#"
((win, s, a) => {
  win['GoogleAnalyticsObject'] = 'ga';
  a = s.createElement('script');
  a.src = 'url';
})(window, document);
"#;
    let expected = r#"
((win, document, a) => {
  win['GoogleAnalyticsObject'] = 'ga';
  a = document.createElement('script');
  a.src = 'url';
})(window, document);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_arg_with_shorter_name_not_renamed() {
    let input = r#"
(function(i, s, a) {
  i['GoogleAnalyticsObject'] = 'ga';
  a = s.createElement('script');
  a.src = 'url';
})(w, document);
"#;
    // ArrowFunction rule converts the IIFE's function expression to an arrow.
    let expected = r#"
((i, document, a) => {
  i['GoogleAnalyticsObject'] = 'ga';
  a = document.createElement('script');
  a.src = 'url';
})(w, document);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
