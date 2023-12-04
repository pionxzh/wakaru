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

type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'silent'

// eslint-disable-next-line no-unused-expressions
yargs(hideBin(process.argv))
    .scriptName('@wakaru/unminify')

    .option('log-level', {
        type: 'string',
        default: 'info',
        choices: ['error', 'warn', 'info', 'debug', 'silent'],
        describe: 'change the level of logging for the CLI.',
    })

    .help()
    .showHelpOnFail(true)
    .alias('h', 'help')

    .version('version', version)
    .alias('v', 'version')

    .usage('Usage: $0 <files...> [options]')
    .command(
        '* <files...>',
        'Unminify your bundled code',
        args => args

            .option('output', {
                alias: 'o',
                describe: 'specify the output directory (default: out/)',
                type: 'string',
                default: 'out/',
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

    // Check if any paths are outside of the current working directory
    for (const p of resolvedPaths) {
        if (!isPathInside(cwd, p)) {
            throw new Error(`File path ${c.green(path.relative(cwd, p))} is outside of the current working directory. This is not allowed.`)
        }
    }

    const outputDir = path.resolve(cwd, output)
    if (await fsa.exists(outputDir)) {
        if (!force) {
            throw new Error(`Output directory already exists at ${c.green(path.relative(cwd, outputDir))}. Pass ${c.yellow('--force')} to overwrite.`)
        }
    }
    await fsa.ensureDir(outputDir)

    const commonBaseDir = findCommonBaseDir(resolvedPaths)
    if (!commonBaseDir) throw new Error('Could not find common base directory')

    for (const p of resolvedPaths) {
        const start = hrtime()

        const source = await fsa.readFile(p, 'utf-8')
        const result = runDefaultTransformation({
            path: p,
            source,
        })
        const outputPath = path.join(outputDir, path.relative(commonBaseDir, p))
        await fsa.ensureDir(path.dirname(outputPath))
        await fsa.writeFile(outputPath, result.code, 'utf-8')

        if (logLevel !== 'silent') {
            const end = hrtime(start)
            const elapsed = end[0] * 1e9 + end[1]
            const formattedElapsed = (elapsed / 1e6).toLocaleString('en-US', { maximumFractionDigits: 1 })
            console.log(`${c.dim('•')} Transforming ${c.green(path.relative(cwd, outputPath))} ${c.dim(`(${formattedElapsed}ms)`)}`)
        }
    }
}

/**
 * Check if base path contains target path
 */
function isPathInside(base: string, target: string): boolean {
    const relative = path.relative(base, target)
    return !relative.startsWith('..') && !path.isAbsolute(relative)
}

function findCommonBaseDir(paths: string[]): string | null {
    if (!paths.length) return null

    const absPaths = paths.map(p => path.resolve(p))
    let commonParts = absPaths[0].split(path.sep)

    for (let i = 1; i < absPaths.length; i++) {
        const parts = absPaths[i].split(path.sep)
        for (let j = 0; j < commonParts.length; j++) {
            if (commonParts[j] !== parts[j]) {
                commonParts = commonParts.slice(0, j)
                break
            }
        }
    }

    const commonPath = commonParts.join(path.sep)
    // if path is not a directory, use its parent directory
    return fsa.statSync(commonPath).isDirectory()
        ? commonPath
        : path.dirname(commonPath)
}
