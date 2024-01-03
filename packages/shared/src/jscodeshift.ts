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
