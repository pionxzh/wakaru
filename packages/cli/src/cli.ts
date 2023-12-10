#!/usr/bin/env node
/* eslint-disable no-console */
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'
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
import fsa from 'fs-extra'
import c from 'picocolors'
import { version } from '../package.json'
import { Concurrency } from './concurrency'
import { findCommonBaseDir, getRelativePath, isPathInside, resolveGlob } from './path'
import { unminify } from './unminify'
import { unpacker } from './unpacker'

enum Feature {
    Unpacker = 'Unpacker',
    Unminify = 'Unminify',
}

async function main() {
    const cwd = process.cwd()

    console.log()
    intro(c.cyan(c.inverse(` Wakaru v${version} `)))

    const features = await multiselect({
        message: `Select features to use ${c.dim('(Use <space> to select, <enter> to submit)')}`,
        options: [
            { label: 'Unpacker - Unpacks bundled code into separated modules', value: Feature.Unpacker },
            { label: 'Unminify - Converts minified code into its readable form', value: Feature.Unminify },
        ],
        initialValues: [Feature.Unpacker],
    })

    if (isCancel(features)) {
        cancel('Cancelled')
        return process.exit(0)
    }

    outro(`Selected features: ${c.green(features.join(', '))}`)

    let unminifyInputPaths: string[] = []

    if (features.includes(Feature.Unpacker)) {
        intro(`${c.green(c.inverse(' Unpacker '))}`)

        const rawInputPath = await text({
            message: `Input file path ${c.dim('(Supports only single file)')}`,
            placeholder: './input.js',
            validate(value) {
                if (!value) return 'Please enter a file path'

                const inputPath = path.resolve(value)
                if (!fsa.existsSync(inputPath)) return 'Input does not exist'
                if (!fsa.statSync(inputPath).isFile()) return 'Input is not a file'

                return undefined
            },
        })

        if (isCancel(rawInputPath)) {
            cancel('Cancelled')
            return process.exit(0)
        }

        const inputPath = path.resolve(rawInputPath)
        const outputPathPlaceholder = './out'

        const rawOutputPath = await text({
            message: `Output directory path ${c.dim('(<enter> to accept default)')}`,
            placeholder: outputPathPlaceholder,
            validate(value) {
                if (!value) return undefined // default value

                const outputPath = path.resolve(value)
                if (!fsa.statSync(outputPath).isDirectory()) return 'Output is not a directory'
                if (!isPathInside(cwd, outputPath)) return 'Output is outside of the current working directory'

                return undefined
            },
        })

        if (isCancel(rawOutputPath)) {
            cancel('Cancelled')
            return process.exit(0)
        }

        const outputPath = path.resolve(rawOutputPath || outputPathPlaceholder)
        const relativeOutputPath = getRelativePath(cwd, outputPath)

        if (fsa.existsSync(outputPath)) {
            const overwrite = await confirm({
                message: `Output directory already exists at ${c.green(relativeOutputPath)}. Overwrite?`,
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

        log.step('Unpacking...')

        const s = spinner()
        s.start('...')
        const items = await unpacker([inputPath], outputPath)
        s.stop('Finished')

        const totalModules = items.reduce((acc, item) => acc + item.modules.length, 0)
        const totalElapsed = items.reduce((acc, item) => acc + item.elapsed, 0)
        const formattedElapsed = (totalElapsed / 1e6).toLocaleString('en-US', { maximumFractionDigits: 1 })
        log.success(`Successfully generated ${c.green(totalModules)} modules ${c.dim(`(${formattedElapsed}ms)`)}`)
        outro(`Output directory: ${c.green(relativeOutputPath)}`)

        unminifyInputPaths = items.flatMap(item => item.files)
    }

    if (features.includes(Feature.Unminify)) {
        intro(`${c.green(c.inverse(' Unminify '))}`)

        const unpacked = features.includes(Feature.Unpacker)

        if (unpacked && unminifyInputPaths.length === 0) {
            // No need to continue if there are no unpacked files
            return process.exit(0)
        }

        if (!unpacked) {
            const rawInputPath = await text({
                message: `Input file path ${c.dim('(Supports glob patterns)')}`,
                placeholder: './*.js',
                validate(value) {
                    if (!value) return 'Please enter a file path'

                    const resolvedPaths = resolveGlob(value)
                    if (resolvedPaths.length === 0) return 'No files matched'

                    return undefined
                },
            })

            if (isCancel(rawInputPath)) {
                cancel('Cancelled')
                return process.exit(0)
            }

            unminifyInputPaths = resolveGlob(rawInputPath)
        }
        else {
            log.success('Input file path')
            log.message(c.dim('Skipped'))
        }

        const commonBaseDir = findCommonBaseDir(unminifyInputPaths)
        if (!commonBaseDir) {
            log.error(`Could not find common base directory with input paths ${c.green(unminifyInputPaths.join(', '))}`)
            return process.exit(1)
        }

        let outputPathPlaceholder = './out'
        if (unpacked) {
            outputPathPlaceholder = getRelativePath(cwd, path.join(commonBaseDir, outputPathPlaceholder))
        }

        const rawOutputPath = await text({
            message: `Output directory path ${c.dim('(<enter> to accept default)')}`,
            placeholder: outputPathPlaceholder,
            validate(value) {
                if (!value) return undefined // default value

                const outputPath = path.resolve(value)
                if (fsa.existsSync(outputPath) && !fsa.statSync(outputPath).isDirectory()) return 'Output path is not a directory'
                if (!isPathInside(cwd, outputPath)) return 'Output path is outside of the current working directory'

                return undefined
            },
        })

        if (isCancel(rawOutputPath)) {
            cancel('Cancelled')
            return process.exit(0)
        }

        const outputPath = path.resolve(rawOutputPath || outputPathPlaceholder)
        const relativeOutputPath = getRelativePath(cwd, outputPath)

        if (fsa.existsSync(outputPath)) {
            const overwrite = await confirm({
                message: `Output directory already exists at ${c.green(relativeOutputPath)}. Overwrite?`,
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

        const concurrency = Math.min(os.cpus().length, 2) // TODO: Make this configurable
        const concurrencyManager = new Concurrency({ concurrency })

        log.step('Unminifying...')

        const s = spinner()
        s.start('...')
        await Promise.all(
            unminifyInputPaths.map(p => concurrencyManager.add(async () => {
                await unminify([p], commonBaseDir, outputPath, true)
                s.message(`${c.green(path.relative(cwd, p))}`)
            })),
        )
        s.stop('Finished')

        log.success(`Successfully unminified ${c.green(unminifyInputPaths.length)} files`)

        outro(`Output directory: ${c.green(relativeOutputPath)}`)
    }

    console.log()
    console.log(`Problems? Please report them at ${c.underline(c.cyan('https://github.com/pionxzh/wakaru/issues'))}`)
    console.log()
}

main().catch(err => console.error(err))
