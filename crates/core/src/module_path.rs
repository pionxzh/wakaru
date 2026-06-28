/// Resolve a relative module specifier (`./x`, `../y/z.js`) written in
/// `from_filename` to the normalized module key it points at. Returns `None`
/// for bare/package specifiers (`react`, `fs`) that do not name a local module.
pub(crate) fn resolve_relative_specifier(from_filename: &str, spec: &str) -> Option<String> {
    if !(spec.starts_with("./") || spec.starts_with("../")) {
        return None;
    }
    let from = from_filename.replace('\\', "/");
    let mut parts: Vec<&str> = from
        .rsplit_once('/')
        .map(|(dir, _)| dir.split('/').filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

pub(crate) fn relative_import_specifier(from_filename: &str, target_filename: &str) -> String {
    let from = from_filename.replace('\\', "/");
    let target = target_filename.replace('\\', "/");
    let from_dir: Vec<&str> = from
        .rsplit_once('/')
        .map(|(dir, _)| dir.split('/').filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    let target_parts: Vec<&str> = target.split('/').filter(|part| !part.is_empty()).collect();

    let mut common = 0usize;
    while common < from_dir.len()
        && common < target_parts.len()
        && from_dir[common] == target_parts[common]
    {
        common += 1;
    }

    let mut parts = Vec::new();
    parts.extend(std::iter::repeat_n("..", from_dir.len() - common));
    parts.extend(target_parts[common..].iter().copied());

    let path = parts.join("/");
    if path.starts_with("../") {
        path
    } else {
        format!("./{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_specifier_is_relative_to_importing_module() {
        assert_eq!(
            relative_import_specifier("module-200.js", "module-100.js"),
            "./module-100.js"
        );
        assert_eq!(
            relative_import_specifier("module-11111.js", "module-11111/chunk_value.js"),
            "./module-11111/chunk_value.js"
        );
        assert_eq!(
            relative_import_specifier("module-22222/chunk_value.js", "module-44444.js"),
            "../module-44444.js"
        );
        assert_eq!(
            relative_import_specifier("module-22222/chunk_value.js", "module-22222/chunk_other.js"),
            "./chunk_other.js"
        );
        assert_eq!(
            relative_import_specifier("module-22222/chunk_value.js", "module-33333/chunk_extra.js"),
            "../module-33333/chunk_extra.js"
        );
        assert_eq!(
            relative_import_specifier("src/index.js", "src/value.js"),
            "./value.js"
        );
    }

    #[test]
    fn relative_specifier_resolves_from_importing_module() {
        assert_eq!(
            resolve_relative_specifier("src/index.js", "./value.js").as_deref(),
            Some("src/value.js")
        );
        assert_eq!(
            resolve_relative_specifier("src/views/index.js", "../value.js").as_deref(),
            Some("src/value.js")
        );
        assert_eq!(
            resolve_relative_specifier("src/index.js", "../../outside.js"),
            None
        );
        assert_eq!(
            resolve_relative_specifier("index.js", "../outside.js"),
            None
        );
        assert_eq!(resolve_relative_specifier("src/index.js", "react"), None);
    }
}
