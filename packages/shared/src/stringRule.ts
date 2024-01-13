import type { BaseTransformationRule, JSCodeshiftTransform } from './rule'
import type { ZodSchema, z } from 'zod'

export type StringTransformation<Schema extends ZodSchema = ZodSchema> = (
    code: string,
    params: z.infer<Schema>
) => Promise<string | void> | string | void

export class StringTransformationRule<Schema extends ZodSchema = ZodSchema> implements BaseTransformationRule {
    type = 'string' as const

    id: string

    name: string

    tags: string[]

    schema?: ZodSchema

    transform: StringTransformation<Schema>

    constructor({
        name, tags = [], transform, schema,
    }: {
        name: string
        tags?: string[]
        transform: StringTransformation<Schema>
        schema?: ZodSchema
    },
    ) {
        this.id = name
        this.name = name
        this.tags = tags
        this.transform = transform
        this.schema = schema
    }

    async execute({
        source, filename, params,
    }: {
        source: string
        filename: string
        params: z.infer<Schema>
    }) {
        try {
            return await this.transform(source, params)
        }
        catch (err: any) {
            console.error(`\nError running rule ${this.name} on ${filename}`, err)
        }
    }

    toJSCodeshiftTransform(): JSCodeshiftTransform {
        const transform: JSCodeshiftTransform = async (file, _api, options) => {
            const { source } = file
            const params = options as z.infer<Schema>
            try {
                const newSource = await this.transform(source, params)
                return newSource ?? source
            }
            catch (err) {
                console.error(`\nError running rule ${this.name} on ${file.path}`, err)
                return null // return null to indicate skip
            }
        }
        return transform
    }

    withId(id: string) {
        const rule = this.clone()
        rule.id = id
        return rule
    }

    private clone() {
        return new StringTransformationRule({
            name: this.name,
            tags: this.tags,
            transform: this.transform,
            schema: this.schema,
        })
    }
}

export const createStringTransformationRule = <Schema extends ZodSchema = ZodSchema>(
    {
        name,
        tags = [],
        transform,
        schema,
    }: {
        name: string
        tags?: string[]
        transform: StringTransformation<Schema>
        schema?: ZodSchema
    },
): StringTransformationRule<Schema> => {
    return new StringTransformationRule({
        name,
        tags,
        transform,
        schema,
    })
}
