import { nonNullable } from '@wakaru/shared/array'
import { jscodeshiftWithParser as j, printSourceWithErrorLoc } from '@wakaru/shared/jscodeshift'
import { Timing } from '@wakaru/shared/timing'
import { basename } from 'pathe'
import { transformationRules } from './transformations'
import { arraify } from './utils/arraify'
import type { MaybeArray } from './utils/arraify'
import type { TransformationRule } from '@wakaru/shared/rule'
import type { Collection, FileInfo } from 'jscodeshift'

export * from './transformations'

export function runDefaultTransformationRules<P extends Record<string, any>>(
    fileInfo: FileInfo,
    params: P = {} as any,
) {
    return executeTransformationRules(fileInfo.source, fileInfo.path, transformationRules, params)
}

export function runTransformationRules<P extends Record<string, any>>(
    fileInfo: FileInfo,
    ruleIds: string[],
    params: P = {} as any,
) {
    const rules = ruleIds.map(id => transformationRules.find(rule => rule.id === id)).filter(nonNullable)
    return executeTransformationRules(fileInfo.source, fileInfo.path, rules, params)
}

function executeTransformationRules<P extends Record<string, any>>(
    /** The source code */
    source: string,
    /** The file path */
    filePath: string,
    rules: MaybeArray<TransformationRule>,
    params: P = {} as any,
) {
    const timing = new Timing()
    /**
     * To minimizes the overhead of parsing and serializing code, we will try to
     * keep the code in jscodeshift AST format as long as possible.
     */

    let currentSource: string | null = null
    let currentRoot: Collection | null = null

    const flattenRules = arraify(rules).flatMap((rule) => {
        if (rule.type === 'rule-set') return rule.rules
        return rule
    })

    let hasError = false
    for (const rule of flattenRules) {
        switch (rule.type) {
            case 'jscodeshift': {
                try {
                    const stopMeasure = timing.startMeasure(filePath, 'jscodeshift-parse')
                    currentRoot ??= j(currentSource ?? source)
                    stopMeasure()
                }
                catch (err: any) {
                    console.error(`\nFailed to parse rule ${filePath} with jscodeshift in rule ${rule.id}`, err)
                    printSourceWithErrorLoc(err, currentSource ?? source)

                    hasError = true
                    break
                }

                const stopMeasure = timing.startMeasure(filePath, rule.id)
                // rule execute already handled error
                rule.execute({
                    root: currentRoot,
                    filename: basename(filePath),
                    params,
                })
                stopMeasure()

                currentSource = null
                break
            }
            case 'string': {
                const stopMeasure1 = timing.startMeasure(filePath, 'jscodeshift-print')
                currentSource ??= currentRoot?.toSource() ?? source
                stopMeasure1()

                try {
                    const stopMeasure2 = timing.startMeasure(filePath, rule.id)
                    currentSource = rule.execute({
                        source: currentSource,
                        filename: filePath,
                        params,
                    }) ?? currentSource
                    stopMeasure2()
                }
                catch (err: any) {
                    console.error(`\nError running rule ${rule.id} on ${filePath}`, err)

                    hasError = true
                }
                currentRoot = null
                break
            }
            default: {
                throw new Error(`Unsupported rule type ${rule.type} from ${rule.id}`)
            }
        }

        // stop if there is an error to prevent further damage
        if (hasError) break
    }

    let code = currentSource
    try {
        code ??= currentRoot?.toSource() ?? source
    }
    catch (err) {
        console.error(`\nFailed to print code ${filePath}`, err)
    }

    return {
        path: filePath,
        code,
        timing,
    }
}
