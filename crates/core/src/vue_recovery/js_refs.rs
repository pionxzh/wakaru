use std::collections::HashSet;

use swc_core::atoms::Atom;

use crate::js_names::is_valid_identifier_name;

pub(super) fn collect_js_unshadowed_ident_refs(source: &str, refs: &mut HashSet<Atom>) {
    let mut scoped_refs = HashSet::new();
    collect_js_ident_refs(source, &mut scoped_refs);
    extend_unshadowed_expr_refs(source, scoped_refs, refs);
}

pub(super) fn collect_js_unshadowed_read_refs(source: &str, refs: &mut HashSet<Atom>) {
    let mut scoped_refs = HashSet::new();
    collect_js_read_refs(source, &mut scoped_refs);
    extend_unshadowed_expr_refs(source, scoped_refs, refs);
}

fn extend_unshadowed_expr_refs(source: &str, scoped_refs: HashSet<Atom>, refs: &mut HashSet<Atom>) {
    let mut shadowed_names = HashSet::new();
    collect_js_arrow_param_names(source, &mut shadowed_names);
    refs.extend(
        scoped_refs
            .into_iter()
            .filter(|name| !shadowed_names.contains(name)),
    );
}

fn collect_js_ident_refs(source: &str, refs: &mut HashSet<Atom>) {
    let chars = source.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if is_ident_start(chars[index]) {
            let start = index;
            index += 1;
            while index < chars.len() && is_ident_continue(chars[index]) {
                index += 1;
            }
            let ident = chars[start..index].iter().collect::<String>();
            refs.insert(Atom::from(ident));
            continue;
        }
        index += 1;
    }
}

fn collect_js_read_refs(source: &str, refs: &mut HashSet<Atom>) {
    let chars = source.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '"' | '\'' | '`' => {
                index = if chars[index] == '`' {
                    collect_template_literal_read_refs(&chars, index, refs)
                } else {
                    skip_quoted_js_string(&chars, index)
                };
                continue;
            }
            ch if is_ident_start(ch) => {
                let start = index;
                index += 1;
                while index < chars.len() && is_ident_continue(chars[index]) {
                    index += 1;
                }
                if js_ident_token_is_read(&chars, start, index) {
                    let ident = chars[start..index].iter().collect::<String>();
                    refs.insert(Atom::from(ident));
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
}

fn collect_template_literal_read_refs(
    chars: &[char],
    start: usize,
    refs: &mut HashSet<Atom>,
) -> usize {
    let mut index = start + 1;
    while index < chars.len() {
        if chars[index] == '\\' {
            index += 2;
            continue;
        }
        if chars[index] == '`' {
            return index + 1;
        }
        if chars[index] == '$' && chars.get(index + 1) == Some(&'{') {
            let expr_start = index + 2;
            if let Some(expr_end) = template_literal_expr_end(chars, expr_start) {
                let expr = chars[expr_start..expr_end].iter().collect::<String>();
                collect_js_read_refs(&expr, refs);
                index = expr_end + 1;
                continue;
            }
        }
        index += 1;
    }
    index
}

fn template_literal_expr_end(chars: &[char], start: usize) -> Option<usize> {
    let mut index = start;
    let mut depth = 1usize;
    while index < chars.len() {
        match chars[index] {
            '"' | '\'' | '`' => {
                index = skip_quoted_js_string(chars, index);
                continue;
            }
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn skip_quoted_js_string(chars: &[char], start: usize) -> usize {
    let quote = chars[start];
    let mut index = start + 1;
    while index < chars.len() {
        if chars[index] == '\\' {
            index += 2;
            continue;
        }
        if chars[index] == quote {
            return index + 1;
        }
        index += 1;
    }
    index
}

fn js_ident_token_is_read(chars: &[char], start: usize, end: usize) -> bool {
    let ident = chars[start..end].iter().collect::<String>();
    if matches!(
        ident.as_str(),
        "true"
            | "false"
            | "null"
            | "undefined"
            | "if"
            | "else"
            | "return"
            | "const"
            | "let"
            | "var"
            | "new"
    ) {
        return false;
    }

    let prev = chars[..start]
        .iter()
        .rposition(|ch| !ch.is_whitespace())
        .map(|index| chars[index]);
    if matches!(prev, Some('.')) {
        return false;
    }

    let next = chars[end..]
        .iter()
        .position(|ch| !ch.is_whitespace())
        .map(|offset| chars[end + offset]);
    !matches!(next, Some(':'))
}

fn collect_js_arrow_param_names(source: &str, names: &mut HashSet<Atom>) {
    let mut cursor = 0;
    while let Some(offset) = source[cursor..].find("=>") {
        let arrow = cursor + offset;
        for name in arrow_param_names(&source[..arrow]) {
            names.insert(Atom::from(name));
        }
        cursor = arrow + 2;
    }
    collect_js_declared_names(source, names);
}

fn arrow_param_names(left: &str) -> Vec<String> {
    let left = left.trim_end();
    if let Some(params) = left.strip_suffix(')') {
        let Some(open) = params.rfind('(') else {
            return Vec::new();
        };
        return params[open + 1..]
            .split(',')
            .map(str::trim)
            .filter(|param| is_valid_identifier_name(param))
            .map(ToString::to_string)
            .collect();
    }

    let end = left.len();
    let start = left
        .char_indices()
        .rev()
        .find_map(|(index, ch)| (!is_ident_continue(ch)).then_some(index + ch.len_utf8()))
        .unwrap_or(0);
    let param = left[start..end].trim();
    is_valid_identifier_name(param)
        .then(|| param.to_string())
        .into_iter()
        .collect()
}

fn collect_js_declared_names(source: &str, names: &mut HashSet<Atom>) {
    let chars = source.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        let Some(keyword_len) = declaration_keyword_len(&chars, index) else {
            index += 1;
            continue;
        };
        index += keyword_len;

        loop {
            while index < chars.len() && chars[index].is_whitespace() {
                index += 1;
            }
            if index >= chars.len() || !is_ident_start(chars[index]) {
                break;
            }

            let start = index;
            index += 1;
            while index < chars.len() && is_ident_continue(chars[index]) {
                index += 1;
            }
            let ident = chars[start..index].iter().collect::<String>();
            names.insert(Atom::from(ident));

            let mut depth = 0usize;
            while index < chars.len() {
                match chars[index] {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => depth = depth.saturating_sub(1),
                    ',' | ';' if depth == 0 => break,
                    _ => {}
                }
                index += 1;
            }
            if index >= chars.len() || chars[index] != ',' {
                break;
            }
            index += 1;
        }
    }
}

fn declaration_keyword_len(chars: &[char], index: usize) -> Option<usize> {
    ["const", "let", "var"].iter().find_map(|keyword| {
        let end = index + keyword.len();
        if end > chars.len() {
            return None;
        }
        let matches_keyword = keyword
            .chars()
            .enumerate()
            .all(|(offset, ch)| chars[index + offset] == ch);
        if !matches_keyword {
            return None;
        }
        let before_ok =
            index == 0 || (!is_ident_continue(chars[index - 1]) && chars[index - 1] != '$');
        let after_ok = end == chars.len() || !is_ident_continue(chars[end]);
        (before_ok && after_ok).then_some(keyword.len())
    })
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}
