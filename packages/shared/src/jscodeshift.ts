import jscodeshift from 'jscodeshift'
import type { API, Collection } from 'jscodeshift'

export const jscodeshiftWithParser = jscodeshift.withParser('babylon')

export const toSource = (root: Collection) => {
    return root.toSource({ lineTerminator: '\n' })
}

const j = jscodeshiftWithParser
export const api: API = {
    j,
    jscodeshift: j,
    stats: () => {},
    report: () => {},
}

export type JSCodeShiftError = Error & { loc?: { line: number; column: number } }

export function printSourceWithErrorLoc(error: JSCodeShiftError, source: string) {
    if (error.loc) {
        const loc = error.loc
        printLine(source, loc.line - 2)
        printLine(source, loc.line - 1)
        printLine(source, loc.line, loc.column)
        printLine(source, loc.line + 1)
        printLine(source, loc.line + 2)
    }
}

function printLine(source: string, line: number, column?: number) {
    const lines = source.split('\n')
    const lineNumber = padLeft(line.toString(), 5, ' ')
    const lineContent = lines[line - 1]
    const linePrefix = `${lineNumber} | `
    console.error(linePrefix + lineContent)

    if (column !== undefined) {
        const linePointer = `${' '.repeat(linePrefix.length + column - 1)}^`
        console.error(linePointer)
    }
}

function padLeft(str: string, len: number, char: string) {
    const count = len > str.length ? len - str.length : 0
    return `${char.repeat(count)}${str}`
}
