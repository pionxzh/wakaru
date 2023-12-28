import path from 'node:path'
import process from 'node:process'
import fsa from 'fs-extra'
import * as globby from 'globby'

/**
 * Check if base path contains target path
 */
export function isPathInside(base: string, target: string): boolean {
    const relative = path.relative(base, target)
    return !relative.startsWith('..') && !path.isAbsolute(relative)
}

/**
 * Get relative path from one path to another.
 *
 * This is a wrapper around `path.relative` that prepends `./` to indicate it's in the current directory.
 *
 * @example
 * path.relative('/a/b', '/a/b/d') // 'd'
 * getRelativePath('/a/b', '/a/b/d') // './d'
 */
export function getRelativePath(from: string, to: string) {
    let relativePath = path.relative(from, to)

    // Check if the path is in the current directory and doesn't start with '.' or '..'
    if (!relativePath.startsWith('.') && !relativePath.startsWith('..')) {
        // Prepend './' to indicate it's in the current directory
        relativePath = `.${path.sep}${relativePath}`
    }

    return relativePath
}

export function findCommonBaseDir(paths: string[]): string | null {
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

export function resolveGlob(glob: string) {
    const cwd = process.cwd()
    glob = path.normalize(glob).replace(/\\/g, '/')
    return globby.sync(glob, {
        cwd: process.cwd(),
        absolute: true,
        ignore: [path.join(cwd, '**/node_modules/**')],
    })
}

export function resolveFileGlob(glob: string) {
    return resolveGlob(glob).filter(p => fsa.existsSync(p) && fsa.statSync(p).isFile())
}

export function pathCompletion({
    // can be a path or a file path or incomplete string
    input,
    baseDir = process.cwd(),
    directoryOnly = false,
}: {
    input: string
    baseDir?: string
    directoryOnly?: boolean
}): string {
    // Determine if the input is an absolute path
    const fullPath = path.isAbsolute(input) ? input : path.resolve(baseDir, input)

    // Get the directory part and the part of the path to be completed
    const dir = path.dirname(fullPath)
    const toComplete = path.basename(fullPath)

    // Check if the directory exists
    if (!fsa.existsSync(dir)) {
        return input
    }

    // Read the directory content
    const files = fsa.readdirSync(dir)

    // Find the first matching file or directory
    const match = files
        .filter((file) => {
            if (directoryOnly && !fsa.statSync(path.join(dir, file)).isDirectory()) {
                return false
            }
            return true
        })
        .find((file) => {
            return file.startsWith(toComplete)
        })

    if (match) {
        const matchedPath = path.join(dir, match)
        const isDir = fsa.statSync(matchedPath).isDirectory()
        // Check if the matched path is a directory and append a slash if it is
        return getRelativePath(baseDir, matchedPath) + (isDir ? path.sep : '')
    }

    return input
}
