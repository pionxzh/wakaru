import path from 'node:path'
import { atom } from 'jotai'
import { atomEffect } from 'jotai-effect'
import { eventEmitter } from '../hooks/useEventEmitter'
import { fsAtom } from './fs'
import type { FS } from './fs'

export interface IFile {
    type: 'file'
    path: string
    name: string
    ext: string
}
export interface IDir {
    type: 'dir'
    path: string
    name: string
    children: IDirent[]
}

export type IDirent = IFile | IDir

function createFile(parent: string, name: string): IFile {
    return {
        type: 'file',
        path: parent === '/' ? `/${name}` : `${parent}/${name}`,
        name,
        ext: path.extname(name),
    }
}

function createDir(parent: string, name: string): IDir {
    return {
        type: 'dir',
        path: parent === '/' ? `/${name}` : `${parent}/${name}`,
        name,
        children: [],
    }
}

async function readDirs(fs: FS, path: string) {
    const result: IDirent[] = []
    const dirs = await fs.promises.readdir(path, {
        recursive: true,
    })

    for (const dir of dirs) {
        const fullPath = path === '/' ? `/${dir}` : `${path}/${dir}`
        if ((await fs.promises.stat(fullPath)).isDirectory()) {
            const _dir = createDir(path, dir)
            _dir.children = await readDirs(fs, fullPath)
            result.push(_dir)
        }
        else {
            result.push(createFile(path, dir))
        }
    }

    return result
}

const fileListTriggerAtom = atom(0)

export const fileListAtom = atom(async (get) => {
    const fs = await get(fsAtom)
    return readDirs(fs, '/')
})

export const fileListUpdateEffect = atomEffect((_get, set) => {
    eventEmitter.on('file:rename', () => {
        set(fileListTriggerAtom, v => v + 1)
    })
})
