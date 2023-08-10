import * as path from 'node:path'
import fsa from 'fs-extra'
import * as globby from 'globby'
import c from 'picocolors'
import yargs from 'yargs'
import { hideBin } from 'yargs/helpers'
import { version } from '../package.json'
import { runDefaultTransformation } from '.'
import type { FileInfo } from 'jscodeshift'
import type { Argv } from 'yargs'

interface UnminifyOptions {
    input: string
    output?: string
}

function commonOptions(args: Argv<{}>): Argv<UnminifyOptions> {
    return args
        .option('input', {
            alias: 'i',
            demandOption: true,
            type: 'string',
            describe: 'specify the input file or directory (glob pattern is supported)',
        })
        .demandOption('input', c.yellow('Please add -i to specify the input file or directory'))
        .option('output', {
            alias: 'o',
            default: 'dist/',
            type: 'string',
            describe: 'specify the output directory (default: dist/)',
        })
}

// eslint-disable-next-line no-unused-expressions
yargs(hideBin(process.argv))
    .scriptName('unminify')
    .usage('Usage: $0 -i <bundle_file> -o <out_dir>`')
    .command(
        '*',
        'Unminify your bundled code',
        args => commonOptions(args).help(),
        async (args) => {
            // const exitCode = await check(await resolveConfig(args))
            // process.exit(exitCode)
        },
    )
    .showHelpOnFail(true)
    .alias('h', 'help')
    .version('version', version)
    .alias('v', 'version')
    .help()
    .argv

export async function codemod(
    paths: string[],
    output: string,
) {
    const cwd = process.cwd()
    const resolvedPaths = globby.sync(paths.concat('!node_modules'))
    const outputPaths: string[] = []
    const outputDir = path.resolve(cwd, output)
    fsa.ensureDirSync(outputDir)

    resolvedPaths.forEach(async (p) => {
        const source = fsa
            .readFileSync(p)
            .toString()
            .split('\r\n')
            .join('\n')

        const fileInfo: FileInfo = {
            path: p,
            source,
        }
        const result = runDefaultTransformation(fileInfo)

        if (source !== result.code) {
            console.log(`Writing file: ${p}`)
            const outputPath = path.join(outputDir, path.relative(cwd, p))
            outputPaths.push(outputPath)
            fsa.ensureDirSync(path.dirname(outputPath))
            fsa.writeFileSync(outputPath, result.code)
        }
    })
}
