#!/usr/bin/env npx tsx

import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { add, complete, cycle, save, suite } from 'benny'
import MagicString from 'magic-string'
import TreeSitterParser from 'web-tree-sitter'
import { runTransformations, transformationMap } from '../packages/unminify/src/index'

const __dirname = dirname(fileURLToPath(import.meta.url))

const title = 'un-undefined'
const rule = transformationMap[title]
const snippet = `
void 0;
void 99;
`

const main = async () => {
    await TreeSitterParser.init()
    const parser = new TreeSitterParser()
    const lang = await TreeSitterParser.Language.load(join(__dirname, './tree-sitter-javascript.wasm'))
    parser.setLanguage(lang)

    await suite(
        title,
        ...([10, 100, 1000].map((count) => {
            const source = snippet.repeat(count)
            return add(`items=${count}`, () => {
                runTransformations({ path: '', source }, [rule])
            })
        })),
        ...([10, 100, 1000, 5000].map((count) => {
            const source = snippet.repeat(count)
            return add(`tree=${count}`, async () => {
                const und = 'undefined'
                const s = new MagicString(source)
                const tree = parser.parse(source)

                const query = lang.query(`
                (unary_expression
                    operator: "void"
                    argument: [
                        (number)
                        (parenthesized_expression
                            (number)
                        )
                    ]
                ) @target
                `)

                query.matches(tree.rootNode).forEach((match) => {
                    match.captures.forEach((capture) => {
                        const node = capture.node
                        s.update(node.startIndex, node.endIndex, und)
                    })
                })

                // query.delete()
                // tree.delete()

                return s.toString()
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
