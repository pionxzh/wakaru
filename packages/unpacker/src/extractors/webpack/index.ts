import { getModulesForWebpackJsonP } from './jsonp'
import { getModulesForWebpack4 } from './webpack4'
import { getModulesForWebpack5 } from './webpack5'
import type { Collection, JSCodeshift } from 'jscodeshift'

export function getModulesFromWebpack(j: JSCodeshift, root: Collection) {
    return getModulesForWebpack5(j, root)
        || getModulesForWebpack4(j, root)
        || getModulesForWebpackJsonP(j, root)
}
