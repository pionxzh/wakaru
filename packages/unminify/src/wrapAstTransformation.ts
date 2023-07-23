import type { Core, JSCodeshift, Options, Transform } from 'jscodeshift'

export interface Context {
    root: ReturnType<Core>
    j: JSCodeshift
    filename: string
}

export interface ASTTransformation<Params = {}> {
    (context: Context, params: Params): string | void
}

export default function astTransformationToJSCodeshiftModule<Params extends Options>(
    transformAST: ASTTransformation<Params>,
): Transform {
    // @ts-expect-error - jscodeshift is not happy
    const transform: Transform = (file, api, options: Params) => {
        const j = api.jscodeshift
        let root
        try {
            root = j(file.source)
        }
        catch (err) {
            console.error(
        `JSCodeshift failed to parse ${file.path},`
          + ' please check whether the syntax is valid',
            )
            console.error(err)

            if (err instanceof SyntaxError && 'loc' in err) {
                const padLeft = (str: string, len: number, char: string) => {
                    return `${char.repeat(len - str.length)}${str}`
                }
                function printLine(line: number, column?: number) {
                    const lines = file.source.split('\n')
                    const lineNumber = padLeft(line.toString(), 3, ' ')
                    const lineContent = lines[line - 1]
                    const linePrefix = `${lineNumber} | `
                    console.error(linePrefix + lineContent)

                    if (column !== undefined) {
                        const linePointer = `${' '.repeat(linePrefix.length + column - 1)}^`
                        console.error(linePointer)
                    }
                }

                const loc: any = err.loc
                printLine(loc.line - 2)
                printLine(loc.line - 1)
                printLine(loc.line, loc.column)
                printLine(loc.line + 1)
                printLine(loc.line + 2)
            }
            return
        }

        try {
            const result = transformAST({ root, j, filename: file.path }, options)
            return result ?? root.toSource({ lineTerminator: '\n' })
        }
        catch (error) {
            console.error(error)
        }
    }

    return transform
}
