#!/usr/bin/env npx tsx

import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { add, complete, cycle, save, suite } from 'benny'
import { runTransformations, transformationMap } from '../packages/unminify/src/index'

const __dirname = dirname(fileURLToPath(import.meta.url))

const title = 'un-undefined'
const rule = transformationMap[title]
const snippet = `
void 0;
void 99;
`

const main = async () => {
    await suite(
        title,
        ...([10, 100, 1000, 5000].map((count) => {
            const source = snippet.repeat(count)
            return add(`items=${count}`, () => {
                runTransformations({ path: '', source }, [rule])
            })
        })),
        cycle(),
        complete(),
        save({
            file: title,
            folder: __dirname,
            format: 'json',
        }),
        save({
            file: title,
            folder: __dirname,
            format: 'chart.html',
        }),
    )
}

main()
