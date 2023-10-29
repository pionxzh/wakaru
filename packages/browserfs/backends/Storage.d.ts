/// <reference types="node" />
import { SyncKeyValueStore, SimpleSyncStore, SyncKeyValueFileSystem, SyncKeyValueRWTransaction } from '../generic/key_value_filesystem';
import { type BackendOptions } from './backend';
/**
 * A synchronous key-value store backed by Storage.
 */
export declare class StorageStore implements SyncKeyValueStore, SimpleSyncStore {
    protected _storage: any;
    name(): string;
    constructor(_storage: any);
    clear(): void;
    beginTransaction(type: string): SyncKeyValueRWTransaction;
    get(key: string): Buffer | undefined;
    put(key: string, data: Buffer, overwrite: boolean): boolean;
    del(key: string): void;
}
export declare namespace StorageFileSystem {
    /**
     * Options to pass to the StorageFileSystem
     */
    interface Options {
        /**
         * The Storage to use. Defaults to globalThis.localStorage.
         */
        storage: Storage;
    }
}
/**
 * A synchronous file system backed by a `Storage` (e.g. localStorage).
 */
export declare class StorageFileSystem extends SyncKeyValueFileSystem {
    static readonly Name = "Storage";
    static Create: any;
    static readonly Options: BackendOptions;
    static isAvailable(storage?: Storage): boolean;
    /**
     * Creates a new Storage file system using the contents of `Storage`.
     */
    constructor({ storage }: StorageFileSystem.Options);
}
