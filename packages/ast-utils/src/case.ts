/**
 * PascalCase
 */
export function pascalCase(str: string): string {
    return str.replace(/(?:^|[-_])(\w)/g, (_, c) => c.toUpperCase())
}
