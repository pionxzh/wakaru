import { jscodeshiftWithParser as j, printSourceWithErrorLoc, toSource } from './jscodeshift'
import type { BaseTransformationRule, SharedParams } from './rule'
import type { Collection, JSCodeshift, Transform } from 'jscodeshift'
import type { ZodSchema, z } from 'zod'

export interface Context {
    root: Collection
    j: JSCodeshift
    filename: string
}

export type JSCodeshiftTransformation<Schema extends ZodSchema = ZodSchema> = (context: Context, params: z.infer<Schema> & SharedParams) => void

export class JSCodeshiftTransformationRule<Schema extends ZodSchema = ZodSchema> implements BaseTransformationRule {
    type = 'jscodeshift' as const

    id: string

    name: string

    tags: string[]

    schema?: ZodSchema

    transform: JSCodeshiftTransformation<z.infer<Schema>>

    constructor({
        name, tags = [], transform, schema,
    }: {
        name: string
        tags?: string[]
        transform: JSCodeshiftTransformation<z.infer<Schema>>
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
        root, filename, params,
    }: {
        root: Collection
        filename: string
        params: z.infer<Schema>
    }) {
        try {
            const context = { root, j, filename }
            this.transform(context, params)
        }
        catch (err: any) {
            console.error(`\nError running rule ${this.name} on ${filename}`, err)

            printSourceWithErrorLoc(err, toSource(root))
        }
    }

    /**
     * Generate a jscodeshift compatible transform
     */
    toJSCodeshiftTransform(): Transform {
        const transform: Transform = (file, api, options) => {
            const root = api.jscodeshift(file.source)

            this.execute({
                root,
                filename: file.path,
                params: options,
            })

            // TODO: return null if no changes were made
            const source = toSource(root)
            return source
        }

        return transform
    }

    withId(id: string) {
        const rule = this.clone()
        rule.id = id
        return rule
    }

    private clone() {
        return new JSCodeshiftTransformationRule({
            name: this.name,
            tags: this.tags,
            transform: this.transform,
            schema: this.schema,
        })
    }
}

/**
 * Alias for JSCodeshiftTransformation
 */
export type ASTTransformation<Schema extends ZodSchema = ZodSchema> = JSCodeshiftTransformation<Schema>

export const createJSCodeshiftTransformationRule = <Schema extends ZodSchema = ZodSchema>(
    {
        name, tags = [], transform, schema,
    }: {
        name: string
        tags?: string[]
        transform: JSCodeshiftTransformation<z.infer<Schema>>
        schema?: ZodSchema
    },
): JSCodeshiftTransformationRule<Schema> => {
    return new JSCodeshiftTransformationRule({
        name,
        tags,
        transform,
        schema,
    })
}
