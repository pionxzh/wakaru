import { runTransformationRules } from '@wakaru/unminify'
import { exposeApi } from 'threads-es/worker'
import type { CodeModParams, TransformedModule } from './types'

const UnminifyAPI = {
    execute: async ({ name, module, transformationRuleIds, moduleMeta, moduleMapping }: CodeModParams) => {
        try {
            const fileInfo = { path: name, source: module.code }
            const params = { moduleMeta, moduleMapping }
            const { code } = await runTransformationRules(fileInfo, transformationRuleIds, params)
            const transformedDep: TransformedModule = { ...module, transformed: code }
            return transformedDep
        }
        catch (e) {
            // We print the error here because it will lose the stack trace after being sent to the main thread
            console.error(e)
            throw e
        }
    },
}

export type UnminifyApiType = typeof UnminifyAPI

exposeApi(UnminifyAPI)
