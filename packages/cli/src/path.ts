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
