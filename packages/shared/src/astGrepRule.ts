import { type SgNode, js } from '@ast-grep/napi'
import MagicString from 'magic-string'
import type { BaseTransformationRule } from './rule'
import type { Transform } from 'jscodeshift'
import type { ZodSchema, z } from 'zod'

export type AstGrepTransformation<Schema extends ZodSchema = ZodSchema> = (root: SgNode, s: MagicString, params: z.infer<Schema>) => MagicString | void

export class AstGrepTransformationRule<Schema extends ZodSchema = ZodSchema> implements BaseTransformationRule {
    type = 'ast-grep' as const

    id: string

    name: string

    tags: string[]

    schema?: ZodSchema

    transform: AstGrepTransformation<Schema>

    constructor({
        name, tags = [], transform, schema,
    }: {
        name: string
        tags?: string[]
        transform: AstGrepTransformation<Schema>
        schema?: ZodSchema
    },
    ) {
        this.id = name
        this.name = name
        this.tags = tags
        this.transform = transform
        this.schema = schema
    }

    execute({
        source, filename, params,
    }: {
        source: string
        filename: string
        params: z.infer<Schema>
    }) {
        try {
            const ast = js.parse(source)
            const root = ast.root()
            const s = new MagicString(source)
            const resultS = this.transform(root, s, params)
            return resultS ? resultS.toString() : source
        }
        catch (err: any) {
            console.error(`\nError running rule ${this.name} on ${filename}`, err)
            return source
        }
    }

    toJSCodeshiftTransform(): Transform {
        const transform: Transform = (file, _api, options) => {
            const { source } = file
            const params = options as z.infer<Schema>
            return this.execute({ source, filename: file.path, params })
        }
        return transform
    }

    withId(id: string) {
        const rule = this.clone()
        rule.id = id
        return rule
    }

    private clone() {
        return new AstGrepTransformationRule({
            name: this.name,
            tags: this.tags,
            transform: this.transform,
            schema: this.schema,
        })
    }
}

export const createAstGrepTransformationRule = <Schema extends ZodSchema = ZodSchema>(
    {
        name,
        tags = [],
        transform,
        schema,
    }: {
        name: string
        tags?: string[]
        transform: AstGrepTransformation<Schema>
        schema?: ZodSchema
    },
): AstGrepTransformationRule<Schema> => {
    return new AstGrepTransformationRule({
        name,
        tags,
        transform,
        schema,
    })
}
