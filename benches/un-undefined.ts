#!/usr/bin/env npx tsx

import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { add, complete, cycle, save, suite } from 'benny'
import { runTransformationRules } from '../packages/unminify/src/index'

const __dirname = dirname(fileURLToPath(import.meta.url))

const title = 'un-undefined'
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
                runTransformationRules({ path: '', source }, [title])
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
