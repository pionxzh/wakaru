import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Restore optional chaining syntax
 *
 * @example
 * // TypeScript
 * foo === null || foo === void 0 ? void 0 : foo.bar
 * // Babel / SWC
 * var _foo;
 * (_foo = foo) === null || _foo === void 0 ? void 0 : _foo.bar;
 * ->
 * foo?.bar
 *
 * @example
 * // Babel
 * var _foo;
 * (_foo = foo) === null || _foo === void 0 || (_foo = _foo.bar) === null || _foo === void 0 ? void 0 : _foo.baz;
 * // SWC
 * var _foo_bar, _foo;
 * (_foo = foo) === null || _foo === void 0 ? void 0 : (_foo_bar = _foo.bar) === null || _foo_bar === void 0 ? void 0 : _foo_bar.baz;
 * // TypeScript
 * (_a = foo === null || foo === void 0 ? void 0 : foo.bar) === null || _a === void 0 ? void 0 : _a.baz
 * ->
 * foo?.bar?.baz
 *
 * @example
 * // Babel / SWC
 *
 * // TypeScript
 * (_b = (_a = foo === null || foo === void 0 ? void 0 : foo.bar) === null || _a === void 0 ? void 0 : _a.baz) === null || _b === void 0 ? void 0 : _b.call(_a)
 * ->
 * foo?.bar()
 */
export const transformAST: ASTTransformation = (context) => {
    // const { root, j } = context

    // find the pattern
    // TODO: implement
}

export default wrap(transformAST)
