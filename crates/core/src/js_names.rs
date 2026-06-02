use swc_core::ecma::ast::{EsReserved, Ident};

pub fn is_valid_identifier_name(name: &str) -> bool {
    Ident::verify_symbol(name).is_ok()
}

pub fn to_valid_identifier_name(name: &str) -> String {
    match Ident::verify_symbol(name) {
        Ok(()) => name.to_string(),
        Err(mut sanitized) => {
            if Ident::verify_symbol(&sanitized).is_ok() {
                return sanitized;
            }
            sanitized.insert(0, '_');
            if Ident::verify_symbol(&sanitized).is_ok() {
                sanitized
            } else {
                "_".to_string()
            }
        }
    }
}

pub fn is_reserved_binding_name(name: &str) -> bool {
    name.is_reserved() || name.is_reserved_in_strict_mode(true) || name.is_reserved_in_strict_bind()
}

/// Stable builtin roots where preserving `Object.foo` / `Math.foo` shape is
/// clearer than destructuring or keeping a minified alias.
pub fn is_stable_builtin_alias_root(name: &str) -> bool {
    matches!(
        name,
        "Object"
            | "Array"
            | "Math"
            | "JSON"
            | "Reflect"
            | "Promise"
            | "Number"
            | "String"
            | "Symbol"
            | "Date"
            | "RegExp"
            | "Map"
            | "Set"
            | "WeakMap"
            | "WeakSet"
            | "Error"
            | "EvalError"
            | "RangeError"
            | "ReferenceError"
            | "SyntaxError"
            | "TypeError"
            | "URIError"
            | "AggregateError"
            | "console"
            | "Proxy"
            | "Intl"
            | "ArrayBuffer"
            | "DataView"
            | "Int8Array"
            | "Uint8Array"
            | "Float32Array"
            | "Float64Array"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        is_reserved_binding_name, is_stable_builtin_alias_root, is_valid_identifier_name,
        to_valid_identifier_name,
    };

    #[test]
    fn validates_identifier_names_with_swc_rules() {
        assert!(is_valid_identifier_name("validName"));
        assert!(is_valid_identifier_name("$value"));
        assert!(!is_valid_identifier_name("default"));
        assert!(!is_valid_identifier_name("await"));
        assert!(!is_valid_identifier_name("data-state"));
        assert!(!is_valid_identifier_name("123abc"));
    }

    #[test]
    fn sanitizes_invalid_identifier_names_with_swc_rules() {
        assert_eq!(to_valid_identifier_name("validName"), "validName");
        assert_eq!(to_valid_identifier_name("default"), "_default");
        assert_eq!(to_valid_identifier_name("data-state"), "datastate");
        assert_eq!(to_valid_identifier_name("123abc"), "_123abc");
    }

    #[test]
    fn detects_module_binding_reserved_names() {
        assert!(is_reserved_binding_name("default"));
        assert!(is_reserved_binding_name("await"));
        assert!(is_reserved_binding_name("eval"));
        assert!(is_reserved_binding_name("arguments"));
        assert!(!is_reserved_binding_name("notReserved"));
    }

    #[test]
    fn keeps_builtin_alias_roots_narrow() {
        assert!(is_stable_builtin_alias_root("Object"));
        assert!(is_stable_builtin_alias_root("Math"));
        assert!(is_stable_builtin_alias_root("TypeError"));
        assert!(!is_stable_builtin_alias_root("document"));
        assert!(!is_stable_builtin_alias_root("process"));
        assert!(!is_stable_builtin_alias_root("Buffer"));
    }
}
