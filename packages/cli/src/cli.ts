#!/usr/bin/env node

/* eslint-disable no-console */
import { availableParallelism } from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import {
    cancel,
    confirm,
    intro,
    isCancel,
    log,
    multiselect,
    outro,
    spinner,
    text,
} from '@clack/prompts'
import { nonNullable } from '@wakaru/shared/array'
import { Timing } from '@wakaru/shared/timing'
import fsa from 'fs-extra'
import c from 'picocolors'
import { FixedThreadPool } from 'poolifier'
import yargs from 'yargs'
import { hideBin } from 'yargs/helpers'
import { version } from '../package.json'
import { findCommonBaseDir, getRelativePath, isPathInside, pathCompletion, resolveFileGlob } from './path'
import { unpacker } from './unpacker'
import type { UnminifyWorkerParams } from './types'
import type { UnpackerItem } from './unpacker'
import type { ModuleMapping, ModuleMeta } from '@wakaru/ast-utils/types'
import type { TimingStat } from '@wakaru/shared/timing'
import type { Module } from '@wakaru/unpacker'

enum Feature {
    Unpacker = 'Unpacker',
    Unminify = 'Unminify',
}

const INPUT_SIZE_WARNING = 1024 * 1024 * 5 // 5MB
const INPUT_SIZE_WARNING_MESSAGE = (filename: string) => `The size of the input file ${c.cyan(filename)} exceeds ${INPUT_SIZE_WARNING / 1024 / 1024}MB. Processing might take longer and consume more memory than usual. In case of an 'Out of Memory' error, consider increasing the maximum old space size by setting the ${c.green('--max-old-space-size')} environment variable.`

const defaultOutputBase = './out/'
const defaultUnpackerOutputFolder = 'unpack'
const defaultUnminifyOutputFolder = 'unminify'

const unminifyWorkerFile = path.join(
    path.dirname(fileURLToPath(import.meta.url)),
    `unminify.worker${path.extname(fileURLToPath(import.meta.url))}`,
)

// eslint-disable-next-line no-unused-expressions
yargs(hideBin(process.argv))
    .scriptName('@wakaru/cli')

    .help()
    .showHelpOnFail(true)
    .alias('h', 'help')

    .version('version', version)
    .alias('v', 'version')

    .option('output', {
        alias: 'o',
        describe: 'Specify the output directory (default: out/)',
        type: 'string',
    })
    .option('unpacker-output', {
        describe: 'Override the output directory for unpacker (default: out/unpack/)',
        type: 'string',
    })
    .option('unminify-output', {
        describe: 'Override the output directory for unminify (default: out/unminify/)',
        type: 'string',
    })
    .option('perf-output', {
        describe: 'Specify the output directory (default: /)',
        type: 'string',
    })
    .option('force', {
        alias: 'f',
        describe: 'Force overwrite output directory',
        type: 'boolean',
    })
    .option('concurrency', {
        describe: 'Maximum number of concurrent tasks (default: 1)',
        type: 'number',
    })
    .option('perf', {
        describe: 'Show performance statistics',
        type: 'boolean',
    })
    .positional('inputs', {
        describe: 'File paths to process (supports glob patterns)',
        type: 'string',
        array: true,
        demandOption: false,
    })

    .usage('Usage: $0 [inputs...] [options]')
    .command(
        '$0 [inputs...]',
        'Interactive mode',
        args => args,
        async (args) => {
            await interactive(args).catch(err => console.error(err))
        },
    )
    .command(
        'all [inputs...]',
        'Process bundled code with all features',
        args => args,
        async (args) => {
            await nonInteractive([Feature.Unpacker, Feature.Unminify], args).catch(err => console.error(err))
        },
    )
    .command(
        ['unpacker [inputs...]', 'unpack [inputs...]'],
        'Unpack bundled code into separated modules',
        args => args,
        async (args) => {
            await nonInteractive([Feature.Unpacker], args).catch(err => console.error(err))
        },
    )
    .command(
        'unminify [inputs...]',
        'Unminify the code into its readable form',
        args => args,
        async (args) => {
            await nonInteractive([Feature.Unminify], args).catch(err => console.error(err))
        },
    )
    .argv

async function interactive({
    inputs: _inputs,
    output: _output,
    force: _force = false,
    concurrency = 1,
    perf,
}: {
    inputs: string[] | undefined
    output: string | undefined
    force: boolean | undefined
    concurrency: number | undefined
    perf: boolean | undefined
}) {
    console.log()
    intro(c.cyan(c.inverse(` Wakaru CLI v${version} `)))

    /**
     * Input validation
     */
    const cwd = process.cwd()

    let _inputPaths: string[] = []
    let outputBase: string | null = null
    let unminifyInputPaths: string[] = []
    let moduleMeta: ModuleMeta = {}
    let moduleMapping: ModuleMapping = {}
    let _overwrite = _force

    if (_inputs) {
        const validationError = getValidateFromPaths(_inputs, inputFileGlobValidation)
        if (validationError) {
            log.error(validationError)
            return process.exit(1)
        }

        _inputPaths = _inputs.map(p => resolveFileGlob(p)).flat()
        if (_inputPaths.length === 0) {
            log.error('No input files matched')
            return process.exit(1)
        }
    }

    if (_output) {
        const validationError = outputFolderValidation(_output)
        if (validationError) {
            log.error(validationError)
            return process.exit(1)
        }

        outputBase = _output
    }

    const minConcurrency = 1
    const maxConcurrency = availableParallelism()
    concurrency = Math.min(maxConcurrency, Math.max(minConcurrency, concurrency))

    log.message(`${c.dim('Run "wakaru --help" for usage options')}`)

    const features = await multiselect({
        message: `Select features to use ${c.dim('(Use <space> to select, <enter> to submit)')}`,
        options: [
            { label: 'Unpacker - Unpacks bundled code into separated modules', value: Feature.Unpacker },
            { label: 'Unminify - Unminify the code into its readable form', value: Feature.Unminify },
        ],
        initialValues: [Feature.Unpacker],
    })

    if (isCancel(features)) {
        cancel('Cancelled')
        return process.exit(0)
    }

    outro(`Selected features: ${c.green(features.join(', '))}`)

    const singleFeature = features.length === 1

    const timing = new Timing()

    if (features.includes(Feature.Unpacker)) {
        intro(`${c.green(c.inverse(' Unpacker '))}`)

        let inputPaths = _inputPaths
        if (_inputPaths.length === 0) {
            const rawInputPath = await text({
                message: `Input file path ${c.dim('(Supports glob patterns, <TAB> to autocomplete)')}`,
                placeholder: './input.js',
                autocomplete(value) {
                    if (typeof value !== 'string') return

                    return pathCompletion({ input: value, baseDir: cwd })
                },
                validate: inputFileGlobValidation,
            })

            if (isCancel(rawInputPath)) {
                cancel('Cancelled')
                return process.exit(0)
            }

            inputPaths = resolveFileGlob(rawInputPath)
        }

        let outputPath = outputBase
            ? singleFeature ? outputBase : path.join(outputBase, defaultUnpackerOutputFolder)
            : ''
        if (!outputBase) {
            const rawOutputBase = await text({
                message: `Output directory path ${c.dim('(<enter> to accept default)')}`,
                placeholder: defaultOutputBase,
                autocomplete(value) {
                    if (typeof value !== 'string') return

                    return pathCompletion({ input: value, baseDir: cwd, directoryOnly: true })
                },
                validate: outputFolderValidation,
            })

            if (isCancel(rawOutputBase)) {
                cancel('Cancelled')
                return process.exit(0)
            }

            outputBase = path.resolve(rawOutputBase ?? defaultOutputBase)
            outputPath = singleFeature
                ? outputBase
                : path.join(outputBase, defaultUnpackerOutputFolder)
        }

        if (!_overwrite && fsa.existsSync(outputBase)) {
            const overwrite = await confirm({
                message: `Output directory already exists at ${c.green(getRelativePath(cwd, outputBase))}. Overwrite?`,
                initialValue: true,
            })

            if (isCancel(overwrite)) {
                cancel('Cancelled')
                return process.exit(0)
            }

            if (!overwrite) {
                cancel('Output directory already exists')
                return process.exit(1)
            }
        }
        _overwrite = true

        for (const inputPath of inputPaths) {
            const fileSize = fsa.statSync(inputPath).size
            if (fileSize > INPUT_SIZE_WARNING) {
                log.warning(INPUT_SIZE_WARNING_MESSAGE(path.relative(cwd, inputPath)))
            }
        }

        const items: UnpackerItem[] = []
        const stopTiming = timing.start()
        for (const inputPath of inputPaths) {
            log.step(`Unpacking ${c.green(path.relative(cwd, inputPath))}`)

            const filename = path.basename(inputPath)
            const stopMeasure = timing.startMeasure(filename, 'unpacker')
            items.push(await unpacker(inputPath, outputPath))
            stopMeasure()
        }
        const elapsed = stopTiming()

        log.step('Finished')

        const totalModules = items.reduce((acc, item) => acc + item.modules.length, 0)
        log.success(`Successfully generated ${c.green(totalModules)} modules ${c.dim(`(${formatElapsed(elapsed)})`)}`)

        outro(`Output directory: ${c.green(getRelativePath(cwd, outputPath))}`)

        unminifyInputPaths = items.flatMap(item => item.files)
        const modules = items.flatMap(item => item.modules)
        moduleMeta = generateModuleMeta(modules)
        moduleMapping = generateModuleMapping(modules)
    }

    if (features.includes(Feature.Unminify)) {
        intro(`${c.green(c.inverse(' Unminify '))}`)

        const unpacked = features.includes(Feature.Unpacker)

        if (unpacked && unminifyInputPaths.length === 0) {
            log.warning('No unpacked files found. This is not your fault, but a bug in Wakaru. Please report this issue.')
            return process.exit(0)
        }

        if (!unpacked) {
            unminifyInputPaths = _inputPaths ?? []
        }

        if (unminifyInputPaths.length === 0) {
            const rawInputPath = await text({
                message: `Input file path ${c.dim('(Supports glob patterns, <TAB> to autocomplete)')}`,
                placeholder: './*.js',
                autocomplete(value) {
                    if (typeof value !== 'string') return

                    return pathCompletion({ input: value, baseDir: cwd })
                },
                validate: inputFileGlobValidation,
            })
            if (isCancel(rawInputPath)) {
                cancel('Cancelled')
                return process.exit(0)
            }

            unminifyInputPaths = resolveFileGlob(rawInputPath)
        }

        const commonBaseDir = findCommonBaseDir(unminifyInputPaths)
        if (!commonBaseDir) {
            log.error('Could not find common base directory with input paths')
            return process.exit(1)
        }

        let outputDir = outputBase
            ? singleFeature ? outputBase : path.join(outputBase, defaultUnminifyOutputFolder)
            : ''
        if (!outputBase) {
            const rawOutputBase = await text({
                message: `Output directory path ${c.dim('(<enter> to accept default)')}`,
                placeholder: defaultOutputBase,
                autocomplete(value) {
                    if (typeof value !== 'string') return

                    return pathCompletion({ input: value, baseDir: cwd, directoryOnly: true })
                },
                validate: outputFolderValidation,
            })

            if (isCancel(rawOutputBase)) {
                cancel('Cancelled')
                return process.exit(0)
            }

            outputBase = path.resolve(rawOutputBase ?? defaultOutputBase)
            outputDir = singleFeature
                ? outputBase
                : path.join(outputBase, defaultUnminifyOutputFolder)
        }

        if (!_overwrite && fsa.existsSync(outputBase)) {
            const overwrite = await confirm({
                message: `Output directory already exists at ${c.green(getRelativePath(cwd, outputBase))}. Overwrite?`,
                initialValue: true,
            })

            if (isCancel(overwrite)) {
                cancel('Cancelled')
                return process.exit(0)
            }

            if (!overwrite) {
                cancel('Output directory already exists')
                return process.exit(1)
            }
        }

        for (const inputPath of unminifyInputPaths) {
            const fileSize = fsa.statSync(inputPath).size
            if (fileSize > INPUT_SIZE_WARNING) {
                log.warning(INPUT_SIZE_WARNING_MESSAGE(path.relative(cwd, inputPath)))
            }
        }

        log.step(`Unminifying... ${c.dim(`(concurrency: ${concurrency})`)}`)

        const s = spinner()
        s.start('...')

        const poolSize = Math.min(concurrency, unminifyInputPaths.length)
        const pool = new FixedThreadPool<UnminifyWorkerParams, Timing | null>(poolSize, unminifyWorkerFile)
        const unminify = async (inputPath: string) => {
            const outputPath = path.join(outputDir, path.relative(commonBaseDir, inputPath))
            const result = await pool.execute({ inputPath, outputPath, moduleMeta, moduleMapping })
            s.message(`${c.green(path.relative(cwd, inputPath))}`)
            return result
        }

        const stopTiming = timing.start()
        const timings = await Promise.all(unminifyInputPaths.map(p => unminify(p)))
        const elapsed = stopTiming()
        timing.merge(...timings.filter(nonNullable))
        pool.destroy()

        s.stop('Finished')

        log.success(`Successfully unminified ${c.green(unminifyInputPaths.length)} files ${c.dim(`(${formatElapsed(elapsed)})`)}`)

        outro(`Output directory: ${c.green(getRelativePath(cwd, outputDir))}`)
    }

    if (perf && outputBase) {
        const measurement = timing.getMeasurement()
        printPerfStats(measurement)
        writePerfStats(measurement, path.join(outputBase, 'perf.json'))
    }

    console.log()
    console.log(`Problems? Please report them at ${c.underline(c.cyan('https://github.com/pionxzh/wakaru/issues'))}`)
    console.log()
}

async function nonInteractive(features: Feature[], {
    inputs: _inputs,
    output: _output,
    'unpacker-output': _unpackerOutput,
    'unminify-output': _unminifyOutput,
    'perf-output': _perfOutput,
    force = false,
    concurrency = 1,
    perf,
}: {
    inputs: string[] | undefined
    output: string | undefined
    'unpacker-output': string | undefined
    'unminify-output': string | undefined
    'perf-output': string | undefined
    force: boolean | undefined
    concurrency: number | undefined
    perf: boolean | undefined
}) {
    console.log()
    intro(c.cyan(c.inverse(` Wakaru CLI v${version} `)))

    /**
     * Input validation
     */
    const cwd = process.cwd()

    if (_inputs === undefined) {
        log.error('No input files specified')
        return process.exit(1)
    }

    const inputValidationError = getValidateFromPaths(_inputs, inputFileGlobValidation)
    if (inputValidationError) {
        log.error(inputValidationError)
        return process.exit(1)
    }

    const inputPaths = _inputs.map(p => resolveFileGlob(p)).flat()
    if (inputPaths.length === 0) {
        log.error('No input files matched')
        return process.exit(1)
    }

    const outputBase = _output ?? defaultOutputBase
    const singleFeature = features.length === 1
    const unpackerOutput = _unpackerOutput ?? (singleFeature ? outputBase : path.join(outputBase, defaultUnpackerOutputFolder))
    const unminifyOutput = _unminifyOutput ?? (singleFeature ? outputBase : path.join(outputBase, defaultUnminifyOutputFolder))

    const perfOutputBase = _perfOutput || (singleFeature
        ? features.includes(Feature.Unpacker) ? unpackerOutput : unminifyOutput
        : findCommonBaseDir([unpackerOutput, unminifyOutput]) ?? outputBase)
    const perfOutputPath = path.join(perfOutputBase, 'perf.json')

    const outputPathsToCheck = []
    if (features.includes(Feature.Unpacker)) outputPathsToCheck.push(unpackerOutput)
    if (features.includes(Feature.Unminify)) outputPathsToCheck.push(unminifyOutput)
    outputPathsToCheck.push(perfOutputPath)

    const outputValidationError = getValidateFromPaths(outputPathsToCheck, outputFolderValidation)
    if (outputValidationError) {
        log.error(outputValidationError)
        return process.exit(1)
    }

    if (!force) {
        outputPathsToCheck.forEach((p) => {
            if (fsa.existsSync(p)) {
                log.error(`Output directory already exists at ${c.green(getRelativePath(cwd, p))}. Pass ${c.green('--force')} to overwrite`)
                return process.exit(1)
            }
        })
    }

    const minConcurrency = 1
    const maxConcurrency = availableParallelism()
    concurrency = Math.min(maxConcurrency, Math.max(minConcurrency, concurrency))

    outro(`Selected features: ${c.green(features.join(', '))}`)

    let unminifyInputPaths: string[] = []
    let moduleMeta: ModuleMeta = {}
    let moduleMapping: ModuleMapping = {}

    const timing = new Timing()

    if (features.includes(Feature.Unpacker)) {
        intro(`${c.green(c.inverse(' Unpacker '))}`)

        const outputPath = path.resolve(unpackerOutput)
        const relativeOutputPath = getRelativePath(cwd, outputPath)

        for (const inputPath of inputPaths) {
            const fileSize = fsa.statSync(inputPath).size
            if (fileSize > INPUT_SIZE_WARNING) {
                log.warning(INPUT_SIZE_WARNING_MESSAGE(path.relative(cwd, inputPath)))
            }
        }

        const items: UnpackerItem[] = []
        const stopTiming = timing.start()
        for (const inputPath of inputPaths) {
            log.step(`Unpacking ${c.green(path.relative(cwd, inputPath))}`)

            const filename = path.basename(inputPath)
            const stopMeasure = timing.startMeasure(filename, 'unpacker')
            items.push(await unpacker(inputPath, outputPath))
            stopMeasure()
        }
        const elapsed = stopTiming()

        log.step('Finished')

        const totalModules = items.reduce((acc, item) => acc + item.modules.length, 0)
        log.success(`Successfully generated ${c.green(totalModules)} modules ${c.dim(`(${formatElapsed(elapsed)})`)}`)
        outro(`Output directory: ${c.green(relativeOutputPath)}`)

        unminifyInputPaths = items.flatMap(item => item.files)
        const modules = items.flatMap(item => item.modules)
        moduleMeta = generateModuleMeta(modules)
        moduleMapping = generateModuleMapping(modules)
    }

    if (features.includes(Feature.Unminify)) {
        intro(`${c.green(c.inverse(' Unminify '))}`)

        const unpacked = features.includes(Feature.Unpacker)
        if (unpacked && unminifyInputPaths.length === 0) {
            log.warning('No unpacked files found. This is not your fault, but a bug in Wakaru. Please report this issue.')
            return process.exit(0)
        }

        if (!unpacked) {
            unminifyInputPaths = inputPaths
        }

        const commonBaseDir = findCommonBaseDir(unminifyInputPaths)
        if (!commonBaseDir) {
            log.error('Could not find common base directory with input paths')
            return process.exit(1)
        }

        const outputDir = path.resolve(unminifyOutput)
        const relativeOutputPath = getRelativePath(cwd, outputDir)

        for (const inputPath of unminifyInputPaths) {
            const fileSize = fsa.statSync(inputPath).size
            if (fileSize > INPUT_SIZE_WARNING) {
                log.warning(INPUT_SIZE_WARNING_MESSAGE(path.relative(cwd, inputPath)))
            }
        }

        log.step(`Unminifying... ${c.dim(`(concurrency: ${concurrency})`)}`)

        const s = spinner()
        s.start('...')

        const poolSize = Math.min(concurrency, unminifyInputPaths.length)
        const pool = new FixedThreadPool<UnminifyWorkerParams, Timing | null>(poolSize, unminifyWorkerFile)
        const unminify = async (inputPath: string) => {
            const outputPath = path.join(outputDir, path.relative(commonBaseDir, inputPath))
            const result = await pool.execute({ inputPath, outputPath, moduleMeta, moduleMapping })
            s.message(`${c.green(path.relative(cwd, inputPath))}`)
            return result
        }

        const stopTiming = timing.start()
        const timings = await Promise.all(unminifyInputPaths.map(p => unminify(p)))
        const elapsed = stopTiming()
        timing.merge(...timings.filter(nonNullable))
        pool.destroy()

        s.stop('Finished')

        log.success(`Successfully unminified ${c.green(unminifyInputPaths.length)} files ${c.dim(`(${formatElapsed(elapsed)})`)}`)

        outro(`Output directory: ${c.green(relativeOutputPath)}`)
    }

    if (perf) {
        const measurements = timing.getMeasurement()
        printPerfStats(measurements)
        writePerfStats(measurements, perfOutputPath)
    }
}

function inputFileGlobValidation(input: string) {
    if (!input) return 'Please enter a file path'

    const cwd = process.cwd()
    if (fsa.existsSync(input) && fsa.statSync(input).isDirectory()) {
        return 'Input is a directory. If you want to include all files in the directory, use a glob pattern (e.g. ./folder/**/*.js)'
    }

    const resolvedPaths = resolveFileGlob(input)
    if (resolvedPaths.length === 0) return 'No files matched'
    if (resolvedPaths.some(p => !isPathInside(cwd, p))) return 'Input files must be inside the current working directory'

    return undefined
}

function outputFolderValidation(input: string) {
    if (!input) return undefined // default value

    const cwd = process.cwd()
    const outputPath = path.resolve(input)
    if (!isPathInside(cwd, outputPath)) return 'Output must be inside the current working directory'
    if (!fsa.existsSync(outputPath)) return undefined // not exist is fine
    if (!fsa.statSync(outputPath).isDirectory()) return 'Output is not a directory'

    return undefined
}

function formatElapsed(elapsed: number) {
    if (elapsed < 1000) return `${~~elapsed}ms`
    if (elapsed < 1000 * 60) return `${(elapsed / 1000).toFixed(2)}s`
    if (elapsed < 1000 * 60 * 60) return `${~~(elapsed / 1000 / 60)}m${~~((elapsed / 1000) % 60)}s`
    return `${~~(elapsed / 1000 / 60 / 60)}h${~~((elapsed / 1000 / 60) % 60)}m${~~((elapsed / 1000) % 60)}s`
}

function printPerfStats(measurements: TimingStat[]) {
    const groupedByRules = measurements
        .reduce<Record<string, number>>((acc, { key, time }) => {
            acc[key] = (acc[key] ?? 0) + time
            return acc
        }, {})
    const table = Object.entries(groupedByRules)
        .map(([key, time]) => ({ key, time: ~~time }))
        .sort((a, b) => a.time - b.time)
    console.log()
    console.table(table, ['key', 'time'])
}

function writePerfStats(measurements: TimingStat[], outputPath: string) {
    fsa.ensureDirSync(path.dirname(outputPath))
    fsa.writeJSONSync(outputPath, measurements, {
        encoding: 'utf-8',
        spaces: 2,
    })

    console.log()
    console.log(`Performance statistics generated at ${c.green(getRelativePath(process.cwd(), outputPath))}`)
    console.log()
}

function generateModuleMeta(modules: Module[]) {
    return modules.reduce<ModuleMeta>((acc, mod) => {
        acc[mod.id] = {
            import: mod.import,
            export: mod.export,
            tags: mod.tags,
        }
        return acc
    }, {})
}

function generateModuleMapping(modules: Module[]) {
    return modules.reduce<ModuleMapping>((acc, mod) => {
        acc[mod.id] = getModuleFileName(mod)
        return acc
    }, {})
}

function getModuleFileName(dep: Module) {
    if (dep.isEntry) {
        if (dep.id === 0) return 'entry.js'
        return `entry-${dep.id}.js`
    }
    return `module-${dep.id}.js`
}

function getValidateFromPaths(paths: string[], validate: (path: string) => string | undefined) {
    for (const path of paths) {
        const error = validate(path)
        if (error) return error
    }
    return undefined
}
