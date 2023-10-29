import { unpack } from '@wakaru/unpacker'
import { configure, fs } from 'browserfs'
import { atom } from 'jotai'
import code from '../../../../testcases/webpack4/dist/index.js?raw'

export const currentProjectIdAtom = atom<string | null>(null)

export const currentWorkDirAtom = atom<string | null>(null)

export type FS = typeof fs

export const fsAtom = atom<Promise<FS>>(async () => {
    // fs can only be mount once
    // hence we need to dispose it when HMR
    if (import.meta.hot) {
        if (!import.meta.hot.data.fs) {
            import.meta.hot.data.fs = fs
        }
        else {
            return import.meta.hot.data.fs
        }
    }

    await configure({
        fs: 'IndexedDB',
    })

    return fs
})

export const unpackedAtom = atom(() => {
    return unpack(code)
})
