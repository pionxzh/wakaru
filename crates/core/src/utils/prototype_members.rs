/// Member names whose static access mutates the receiver's prototype surface
/// instead of behaving like an ordinary own property.
///
/// `obj.__proto__ = value` triggers the `Object.prototype.__proto__` setter
/// and changes lookup for every name that is not an own property, while
/// `obj.__defineGetter__(...)` / `obj.__defineSetter__(...)` install accessors
/// under names that never surface as static member writes. Any proof that
/// treats a static `exports.name` access as evidence about an ordinary named
/// export must fail closed on these names.
pub(crate) fn is_prototype_mutating_member_name(name: &str) -> bool {
    matches!(name, "__proto__" | "__defineGetter__" | "__defineSetter__")
}
