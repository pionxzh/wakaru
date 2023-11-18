#!/usr/bin/env node

/* eslint-disable no-console */
import * as path from 'node:path'
import process, { hrtime } from 'node:process'
import fsa from 'fs-extra'
import * as globby from 'globby'
import c from 'picocolors'
import yargs from 'yargs'
import { hideBin } from 'yargs/helpers'
import { version } from '../package.json'
import { runDefaultTransformation } from '.'
import type { FileInfo } from 'jscodeshift'
import type { Argv } from 'yargs'

type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'silent'

function commonOptions(args: Argv<unknown>) {
    return args
        .option('log-level', {
            type: 'string',
            default: 'info',
            choices: ['error', 'warn', 'info', 'debug', 'silent'],
            describe: 'change the level of logging for the CLI.',
        })
}

// eslint-disable-next-line no-unused-expressions
yargs(hideBin(process.argv))
    .scriptName('@wakaru/unminify')

    .help()
    .showHelpOnFail(true)
    .alias('h', 'help')

    .version('version', version)
    .alias('v', 'version')

    .usage('Usage: $0 <files...> [options]')
    .command(
        '* <files...>',
        'Unminify your bundled code',
        args => commonOptions(args)
            .option('output', {
                alias: 'o',
                describe: 'specify the output directory (default: dist/)',
                type: 'string',
                default: 'dist/',
            })
            .option('force', {
                alias: 'f',
                describe: 'force overwrite output directory',
                type: 'boolean',
                default: false,
            })
            .positional('files', {
                describe: 'File paths to process (supports glob patterns)',
                type: 'string',
                array: true,
            })
            .help(),
        async (args) => {
            await codemod(
                args.files ?? [],
                args.output,
                args.force,
                args.logLevel as LogLevel,
            )
        },
    )
    .argv

async function codemod(
    paths: string[],
    output: string,
    force: boolean,
    logLevel: LogLevel,
) {
    const cwd = process.cwd()
    const globbyPaths = paths
        .map(p => path.normalize(p))
        .map(p => p.replace(/\\/g, '/'))
    const resolvedPaths = await globby.default(globbyPaths, {
        cwd,
        absolute: true,
        ignore: [path.join(cwd, '**/node_modules/**')],
    })
    const outputDir = path.resolve(cwd, output)

    if (fsa.existsSync(outputDir)) {
        if (!force) {
            throw new Error(`Output directory already exists at ${c.green(path.relative(cwd, outputDir))}. Pass ${c.yellow('--force')} to overwrite.`)
        }
    }
    else {
        fsa.ensureDirSync(outputDir)
    }

    for (const p of resolvedPaths) {
        const start = hrtime()

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
        // remove leading ../
        const safePath = path.relative(cwd, p).replace(/\.\.\\/g, '')
        const outputPath = path.join(outputDir, safePath)
        fsa.ensureDirSync(path.dirname(outputPath))
        fsa.writeFileSync(outputPath, result.code)

        if (logLevel !== 'silent') {
            const end = hrtime(start)
            const elapsed = end[0] * 1e9 + end[1]
            const formattedElapsed = (elapsed / 1e6).toLocaleString('en-US', { maximumFractionDigits: 1 })
            console.log(`${c.dim('•')} Transforming ${c.green(path.relative(cwd, p))} ${c.dim(`(${formattedElapsed}ms)`)}`)
        }
    }
}
