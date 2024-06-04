import { Biome, Distribution } from '@biomejs/js-api'
import { createStringTransformationRule } from '@wakaru/shared/rule'

type FormatConfiguration = Parameters<Biome['applyConfiguration']>[0]

let biome: Biome | null = null

const formatConfig: FormatConfiguration = {
    formatter: {
        enabled: true,
        indentStyle: 'space',
        indentWidth: 2,
        lineWidth: 80,
    },
    javascript: {
        formatter: {
            quoteStyle: 'double',
            jsxQuoteStyle: 'double',
            // trailing_comma: 'all',
            trailingComma: 'es5',
            semicolons: 'always',
            arrowParentheses: 'always',
            bracketSameLine: false,
            bracketSpacing: true,
            enabled: true,
        },
    },
}

/**
 * @url https://github.com/biomejs/biome
 */
export default createStringTransformationRule({
    name: 'prettier',
    transform: async (code) => {
        if (!biome) {
            biome = await Biome.create({
                distribution: Distribution.NODE,
            })
            biome.applyConfiguration(formatConfig)
        }

        return biome.formatContent(code, { filePath: 'example.js' }).content
    },
})
