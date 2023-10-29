/// <reference types="node" />
/**
 * Grab bag of utility functions used across the code.
 */
import { FileSystem } from './filesystem';
import { Cred } from './cred';
import type { BaseBackendConstructor } from './backends/backend';
/**
 * Synchronous recursive makedir.
 * @hidden
 */
export declare function mkdirpSync(p: string, mode: number, cred: Cred, fs: FileSystem): void;
/**
 * Copies a slice of the given buffer
 * @hidden
 */
export declare function copyingSlice(buff: Buffer, start?: number, end?: number): Buffer;
/**
 * Option validator for a Buffer file system option.
 * @hidden
 */
export declare function bufferValidator(v: object): Promise<void>;
/**
 * Checks that the given options object is valid for the file system options.
 * @hidden
 */
export declare function checkOptions(backend: BaseBackendConstructor, opts: object): Promise<void>;
/** Waits n ms.  */
export declare function wait(ms: number): Promise<void>;
/**
 * Converts a callback into a promise. Assumes last parameter is the callback
 * @todo Look at changing resolve value from cbArgs[0] to include other callback arguments?
 */
export declare function toPromise(fn: (...fnArgs: unknown[]) => unknown): (...args: unknown[]) => Promise<unknown>;
/**
 * @hidden
 */
export declare const setImmediate: typeof globalThis.setImmediate | ((cb: any) => NodeJS.Timeout);
