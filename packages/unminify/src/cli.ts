import type { Argv } from 'yargs'
import yargs from 'yargs'
import { hideBin } from 'yargs/helpers'
import c from 'picocolors'
import { version } from '../package.json'

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
