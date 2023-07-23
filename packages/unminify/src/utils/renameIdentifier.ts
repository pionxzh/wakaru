import type { Context } from '../wrapAstTransformation'

export function renameIdentifier(
    context: Context,
    from: string,
    to: string,
): void {
    const { root, j } = context
    root.findVariableDeclarators(from).renameTo(to)
}
