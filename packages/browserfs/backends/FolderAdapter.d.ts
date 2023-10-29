import { BaseFileSystem, type FileSystem } from '../filesystem';
import { type BackendOptions } from './backend';
export declare namespace FolderAdapter {
    /**
     * Configuration options for a FolderAdapter file system.
     */
    interface Options {
        folder: string;
        wrapped: FileSystem;
    }
}
/**
 * The FolderAdapter file system wraps a file system, and scopes all interactions to a subfolder of that file system.
 *
 * Example: Given a file system `foo` with folder `bar` and file `bar/baz`...
 *
 * ```javascript
 * BrowserFS.configure({
 *   fs: "FolderAdapter",
 *   options: {
 *     folder: "bar",
 *     wrapped: foo
 *   }
 * }, function(e) {
 *   var fs = BrowserFS.BFSRequire('fs');
 *   fs.readdirSync('/'); // ['baz']
 * });
 * ```
 */
export declare class FolderAdapter extends BaseFileSystem {
    static readonly Name = "FolderAdapter";
    static Create: any;
    static readonly Options: BackendOptions;
    static isAvailable(): boolean;
    _wrapped: FileSystem;
    _folder: string;
    constructor({ folder, wrapped }: FolderAdapter.Options);
    get metadata(): {
        supportsLinks: boolean;
        name: string;
        readonly: boolean;
        synchronous: boolean;
        supportsProperties: boolean;
        totalSpace: number;
        freeSpace: number;
    };
    /**
     * Initialize the file system. Ensures that the wrapped file system
     * has the given folder.
     */
    private _initialize;
}
