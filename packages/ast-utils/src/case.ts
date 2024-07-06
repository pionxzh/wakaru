/**
 * PascalCase
 *
 * @example
 * pascalCase('foo') // Foo
 * pascalCase('foo-bar') // FooBar
 * pascalCase('foo_bar') // FooBar
 */
export function pascalCase(str: string): string {
    return str.replace(/(?:^|[-_])(\w)/g, (_, c) => c.toUpperCase())
}
