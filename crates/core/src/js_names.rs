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

pub fn is_likely_generated_alias(name: &str) -> bool {
    name.chars().count() <= 2
        || is_numbered_short_alpha_alias(name)
        || is_short_prefixed_generated_alias(name)
}

fn is_numbered_short_alpha_alias(name: &str) -> bool {
    let mut alpha_count = 0;
    let mut saw_digit = false;
    for ch in name.chars() {
        if saw_digit {
            if !ch.is_ascii_digit() {
                return false;
            }
            continue;
        }
        if ch.is_ascii_alphabetic() {
            alpha_count += 1;
            if alpha_count > 3 {
                return false;
            }
        } else if ch.is_ascii_digit() {
            saw_digit = true;
        } else {
            return false;
        }
    }
    (2..=3).contains(&alpha_count) && saw_digit
}

fn is_short_prefixed_generated_alias(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('_').or_else(|| name.strip_prefix('$')) else {
        return false;
    };
    let mut alpha_count = 0;
    let mut saw_digit = false;
    for ch in rest.chars() {
        if saw_digit {
            if !ch.is_ascii_digit() {
                return false;
            }
            continue;
        }
        if ch.is_ascii_alphabetic() {
            alpha_count += 1;
            if alpha_count > 3 {
                return false;
            }
        } else if ch.is_ascii_digit() {
            saw_digit = true;
        } else {
            return false;
        }
    }
    (1..=3).contains(&alpha_count)
}

/// Standard global roots that are safe and useful for SmartInline alias recovery.
///
/// This is intentionally not a full known-global/environment list. SmartInline uses
/// it to decide when aliases like `const d = Object.defineProperty` can be restored
/// to `Object.defineProperty(...)`, and when property-access groups should not be
/// destructured from builtin roots. Keep this limited to stable constructor/namespace
/// roots where re-reading the global is clearer and low-risk.
///
/// Do not add platform/env globals (`document`, `process`, `Buffer`, test globals)
/// or broad global functions/constants (`parseInt`, `eval`, `NaN`, `undefined`,
/// `globalThis`) without a specific SmartInline regression that proves the rewrite
/// is desirable.
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
            | "Boolean"
            | "BigInt"
            | "Symbol"
            | "Date"
            | "RegExp"
            | "Map"
            | "Set"
            | "WeakMap"
            | "WeakSet"
            | "Error"
            | "Function"
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
            | "Atomics"
            | "SharedArrayBuffer"
            | "ArrayBuffer"
            | "DataView"
            | "Int8Array"
            | "Int16Array"
            | "Int32Array"
            | "Uint8Array"
            | "Uint8ClampedArray"
            | "Uint16Array"
            | "Uint32Array"
            | "BigInt64Array"
            | "BigUint64Array"
            | "Float16Array"
            | "Float32Array"
            | "Float64Array"
            | "WeakRef"
            | "FinalizationRegistry"
            | "Iterator"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        is_likely_generated_alias, is_reserved_binding_name, is_stable_builtin_alias_root,
        is_valid_identifier_name, to_valid_identifier_name,
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
    fn classifies_generated_alias_shapes() {
        for name in [
            "a", "ab", "ab1", "abc12", "_a", "_a2", "_ref", "$e", "$ref2",
        ] {
            assert!(is_likely_generated_alias(name), "{name}");
        }

        for name in [
            "user2",
            "label1",
            "foo_1",
            "primary",
            "userId",
            "rootClassName",
        ] {
            assert!(!is_likely_generated_alias(name), "{name}");
        }
    }

    #[test]
    fn keeps_builtin_alias_roots_narrow() {
        assert!(is_stable_builtin_alias_root("Object"));
        assert!(is_stable_builtin_alias_root("Math"));
        assert!(is_stable_builtin_alias_root("TypeError"));
        assert!(is_stable_builtin_alias_root("BigInt"));
        assert!(is_stable_builtin_alias_root("Atomics"));
        assert!(is_stable_builtin_alias_root("FinalizationRegistry"));
        assert!(!is_stable_builtin_alias_root("document"));
        assert!(!is_stable_builtin_alias_root("process"));
        assert!(!is_stable_builtin_alias_root("Buffer"));
        assert!(!is_stable_builtin_alias_root("parseInt"));
        assert!(!is_stable_builtin_alias_root("globalThis"));
        assert!(!is_stable_builtin_alias_root("eval"));
    }
}
