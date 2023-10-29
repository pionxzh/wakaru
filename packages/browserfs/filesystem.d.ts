/// <reference types="node" />
import { ApiError } from './ApiError';
import { Stats } from './stats';
import { File, FileFlag } from './file';
import { Cred } from './cred';
export type BFSOneArgCallback = (e?: ApiError) => unknown;
export type BFSCallback<T> = (e?: ApiError, rv?: T) => unknown;
export type BFSThreeArgCallback<T, U> = (e?: ApiError, arg1?: T, arg2?: U) => unknown;
export type FileContents = Buffer | string;
/**
 * Metadata about a FileSystem
 */
export interface FileSystemMetadata {
    /**
     * The name of the FS
     */
    name: string;
    /**
     * Wheter the FS is readonly or not
     */
    readonly: boolean;
    /**
     * Does the FS support synchronous operations
     */
    synchronous: boolean;
    /**
     * Does the FS support properties
     */
    supportsProperties: boolean;
    /**
     * Does the FS support links
     */
    supportsLinks: boolean;
    /**
     * The total space
     */
    totalSpace: number;
    /**
     * The available space
     */
    freeSpace: number;
}
/**
 * Structure for a filesystem. **All** BrowserFS FileSystems must implement
 * this.
 *
 * ### Argument Assumptions
 *
 * You can assume the following about arguments passed to each API method:
 *
 * - Every path is an absolute path. `.`, `..`, and other items
 *   are resolved into an absolute form.
 * - All arguments are present. Any optional arguments at the Node API level
 *   have been passed in with their default values.
 */
export declare abstract class FileSystem {
    static readonly Name: string;
    abstract readonly metadata: FileSystemMetadata;
    constructor(options?: object);
    abstract whenReady(): Promise<this>;
    /**
     * Asynchronous access.
     */
    abstract access(p: string, mode: number, cred: Cred): Promise<void>;
    /**
     * Synchronous access.
     */
    abstract accessSync(p: string, mode: number, cred: Cred): void;
    /**
     * Asynchronous rename. No arguments other than a possible exception
     * are given to the completion callback.
     */
    abstract rename(oldPath: string, newPath: string, cred: Cred): Promise<void>;
    /**
     * Synchronous rename.
     */
    abstract renameSync(oldPath: string, newPath: string, cred: Cred): void;
    /**
     * Asynchronous `stat`.
     */
    abstract stat(p: string, cred: Cred): Promise<Stats>;
    /**
     * Synchronous `stat`.
     */
    abstract statSync(p: string, cred: Cred): Stats;
    /**
     * Asynchronous file open.
     * @see http://www.manpagez.com/man/2/open/
     * @param flags Handles the complexity of the various file
     *   modes. See its API for more details.
     * @param mode Mode to use to open the file. Can be ignored if the
     *   filesystem doesn't support permissions.
     */
    abstract open(p: string, flag: FileFlag, mode: number, cred: Cred): Promise<File>;
    /**
     * Synchronous file open.
     * @see http://www.manpagez.com/man/2/open/
     * @param flags Handles the complexity of the various file
     *   modes. See its API for more details.
     * @param mode Mode to use to open the file. Can be ignored if the
     *   filesystem doesn't support permissions.
     */
    abstract openSync(p: string, flag: FileFlag, mode: number, cred: Cred): File;
    /**
     * Asynchronous `unlink`.
     */
    abstract unlink(p: string, cred: Cred): Promise<void>;
    /**
     * Synchronous `unlink`.
     */
    abstract unlinkSync(p: string, cred: Cred): void;
    /**
     * Asynchronous `rmdir`.
     */
    abstract rmdir(p: string, cred: Cred): Promise<void>;
    /**
     * Synchronous `rmdir`.
     */
    abstract rmdirSync(p: string, cred: Cred): void;
    /**
     * Asynchronous `mkdir`.
     * @param mode Mode to make the directory using. Can be ignored if
     *   the filesystem doesn't support permissions.
     */
    abstract mkdir(p: string, mode: number, cred: Cred): Promise<void>;
    /**
     * Synchronous `mkdir`.
     * @param mode Mode to make the directory using. Can be ignored if
     *   the filesystem doesn't support permissions.
     */
    abstract mkdirSync(p: string, mode: number, cred: Cred): void;
    /**
     * Asynchronous `readdir`. Reads the contents of a directory.
     *
     * The callback gets two arguments `(err, files)` where `files` is an array of
     * the names of the files in the directory excluding `'.'` and `'..'`.
     */
    abstract readdir(p: string, cred: Cred): Promise<string[]>;
    /**
     * Synchronous `readdir`. Reads the contents of a directory.
     */
    abstract readdirSync(p: string, cred: Cred): string[];
    /**
     * Test whether or not the given path exists by checking with
     * the file system. Then call the callback argument with either true or false.
     */
    abstract exists(p: string, cred: Cred): Promise<boolean>;
    /**
     * Test whether or not the given path exists by checking with
     * the file system.
     */
    abstract existsSync(p: string, cred: Cred): boolean;
    /**
     * Asynchronous `realpath`. The callback gets two arguments
     * `(err, resolvedPath)`.
     *
     * Note that the Node API will resolve `path` to an absolute path.
     * @param cache An object literal of mapped paths that can be used to
     *   force a specific path resolution or avoid additional `fs.stat` calls for
     *   known real paths. If not supplied by the user, it'll be an empty object.
     */
    abstract realpath(p: string, cred: Cred): Promise<string>;
    /**
     * Synchronous `realpath`.
     *
     * Note that the Node API will resolve `path` to an absolute path.
     * @param cache An object literal of mapped paths that can be used to
     *   force a specific path resolution or avoid additional `fs.stat` calls for
     *   known real paths. If not supplied by the user, it'll be an empty object.
     */
    abstract realpathSync(p: string, cred: Cred): string;
    /**
     * Asynchronous `truncate`.
     */
    abstract truncate(p: string, len: number, cred: Cred): Promise<void>;
    /**
     * Synchronous `truncate`.
     */
    abstract truncateSync(p: string, len: number, cred: Cred): void;
    /**
     * Asynchronously reads the entire contents of a file.
     * @param encoding If non-null, the file's contents should be decoded
     *   into a string using that encoding. Otherwise, if encoding is null, fetch
     *   the file's contents as a Buffer.
     * If no encoding is specified, then the raw buffer is returned.
     */
    abstract readFile(fname: string, encoding: BufferEncoding | null, flag: FileFlag, cred: Cred): Promise<FileContents>;
    /**
     * Synchronously reads the entire contents of a file.
     * @param encoding If non-null, the file's contents should be decoded
     *   into a string using that encoding. Otherwise, if encoding is null, fetch
     *   the file's contents as a Buffer.
     */
    abstract readFileSync(fname: string, encoding: BufferEncoding | null, flag: FileFlag, cred: Cred): FileContents;
    /**
     * Asynchronously writes data to a file, replacing the file
     * if it already exists.
     *
     * The encoding option is ignored if data is a buffer.
     */
    abstract writeFile(fname: string, data: FileContents, encoding: BufferEncoding | null, flag: FileFlag, mode: number, cred: Cred): Promise<void>;
    /**
     * Synchronously writes data to a file, replacing the file
     * if it already exists.
     *
     * The encoding option is ignored if data is a buffer.
     */
    abstract writeFileSync(fname: string, data: FileContents, encoding: BufferEncoding | null, flag: FileFlag, mode: number, cred: Cred): void;
    /**
     * Asynchronously append data to a file, creating the file if
     * it not yet exists.
     */
    abstract appendFile(fname: string, data: FileContents, encoding: BufferEncoding | null, flag: FileFlag, mode: number, cred: Cred): Promise<void>;
    /**
     * Synchronously append data to a file, creating the file if
     * it not yet exists.
     */
    abstract appendFileSync(fname: string, data: FileContents, encoding: BufferEncoding | null, flag: FileFlag, mode: number, cred: Cred): void;
    /**
     * Asynchronous `chmod`.
     */
    abstract chmod(p: string, mode: number, cred: Cred): Promise<void>;
    /**
     * Synchronous `chmod`.
     */
    abstract chmodSync(p: string, mode: number, cred: Cred): void;
    /**
     * Asynchronous `chown`.
     */
    abstract chown(p: string, new_uid: number, new_gid: number, cred: Cred): Promise<void>;
    /**
     * Synchronous `chown`.
     */
    abstract chownSync(p: string, new_uid: number, new_gid: number, cred: Cred): void;
    /**
     * Change file timestamps of the file referenced by the supplied
     * path.
     */
    abstract utimes(p: string, atime: Date, mtime: Date, cred: Cred): Promise<void>;
    /**
     * Change file timestamps of the file referenced by the supplied
     * path.
     */
    abstract utimesSync(p: string, atime: Date, mtime: Date, cred: Cred): void;
    /**
     * Asynchronous `link`.
     */
    abstract link(srcpath: string, dstpath: string, cred: Cred): Promise<void>;
    /**
     * Synchronous `link`.
     */
    abstract linkSync(srcpath: string, dstpath: string, cred: Cred): void;
    /**
     * Asynchronous `symlink`.
     * @param type can be either `'dir'` or `'file'`
     */
    abstract symlink(srcpath: string, dstpath: string, type: string, cred: Cred): Promise<void>;
    /**
     * Synchronous `symlink`.
     * @param type can be either `'dir'` or `'file'`
     */
    abstract symlinkSync(srcpath: string, dstpath: string, type: string, cred: Cred): void;
    /**
     * Asynchronous readlink.
     */
    abstract readlink(p: string, cred: Cred): Promise<string>;
    /**
     * Synchronous readlink.
     */
    abstract readlinkSync(p: string, cred: Cred): string;
}
/**
 * Basic filesystem class. Most filesystems should extend this class, as it
 * provides default implementations for a handful of methods.
 */
export declare class BaseFileSystem extends FileSystem {
    static readonly Name: string;
    protected _ready: Promise<this>;
    constructor(options?: {
        [key: string]: unknown;
    });
    get metadata(): FileSystemMetadata;
    whenReady(): Promise<this>;
    /**
     * Opens the file at path p with the given flag. The file must exist.
     * @param p The path to open.
     * @param flag The flag to use when opening the file.
     */
    openFile(p: string, flag: FileFlag, cred: Cred): Promise<File>;
    /**
     * Create the file at path p with the given mode. Then, open it with the given
     * flag.
     */
    createFile(p: string, flag: FileFlag, mode: number, cred: Cred): Promise<File>;
    open(p: string, flag: FileFlag, mode: number, cred: Cred): Promise<File>;
    access(p: string, mode: number, cred: Cred): Promise<void>;
    accessSync(p: string, mode: number, cred: Cred): void;
    rename(oldPath: string, newPath: string, cred: Cred): Promise<void>;
    renameSync(oldPath: string, newPath: string, cred: Cred): void;
    stat(p: string, cred: Cred): Promise<Stats>;
    statSync(p: string, cred: Cred): Stats;
    /**
     * Opens the file at path p with the given flag. The file must exist.
     * @param p The path to open.
     * @param flag The flag to use when opening the file.
     * @return A File object corresponding to the opened file.
     */
    openFileSync(p: string, flag: FileFlag, cred: Cred): File;
    /**
     * Create the file at path p with the given mode. Then, open it with the given
     * flag.
     */
    createFileSync(p: string, flag: FileFlag, mode: number, cred: Cred): File;
    openSync(p: string, flag: FileFlag, mode: number, cred: Cred): File;
    unlink(p: string, cred: Cred): Promise<void>;
    unlinkSync(p: string, cred: Cred): void;
    rmdir(p: string, cred: Cred): Promise<void>;
    rmdirSync(p: string, cred: Cred): void;
    mkdir(p: string, mode: number, cred: Cred): Promise<void>;
    mkdirSync(p: string, mode: number, cred: Cred): void;
    readdir(p: string, cred: Cred): Promise<string[]>;
    readdirSync(p: string, cred: Cred): string[];
    exists(p: string, cred: Cred): Promise<boolean>;
    existsSync(p: string, cred: Cred): boolean;
    realpath(p: string, cred: Cred): Promise<string>;
    realpathSync(p: string, cred: Cred): string;
    truncate(p: string, len: number, cred: Cred): Promise<void>;
    truncateSync(p: string, len: number, cred: Cred): void;
    readFile(fname: string, encoding: BufferEncoding | null, flag: FileFlag, cred: Cred): Promise<FileContents>;
    readFileSync(fname: string, encoding: BufferEncoding | null, flag: FileFlag, cred: Cred): FileContents;
    writeFile(fname: string, data: FileContents, encoding: BufferEncoding | null, flag: FileFlag, mode: number, cred: Cred): Promise<void>;
    writeFileSync(fname: string, data: FileContents, encoding: BufferEncoding | null, flag: FileFlag, mode: number, cred: Cred): void;
    appendFile(fname: string, data: FileContents, encoding: BufferEncoding | null, flag: FileFlag, mode: number, cred: Cred): Promise<void>;
    appendFileSync(fname: string, data: FileContents, encoding: BufferEncoding | null, flag: FileFlag, mode: number, cred: Cred): void;
    chmod(p: string, mode: number, cred: Cred): Promise<void>;
    chmodSync(p: string, mode: number, cred: Cred): void;
    chown(p: string, new_uid: number, new_gid: number, cred: Cred): Promise<void>;
    chownSync(p: string, new_uid: number, new_gid: number, cred: Cred): void;
    utimes(p: string, atime: Date, mtime: Date, cred: Cred): Promise<void>;
    utimesSync(p: string, atime: Date, mtime: Date, cred: Cred): void;
    link(srcpath: string, dstpath: string, cred: Cred): Promise<void>;
    linkSync(srcpath: string, dstpath: string, cred: Cred): void;
    symlink(srcpath: string, dstpath: string, type: string, cred: Cred): Promise<void>;
    symlinkSync(srcpath: string, dstpath: string, type: string, cred: Cred): void;
    readlink(p: string, cred: Cred): Promise<string>;
    readlinkSync(p: string, cred: Cred): string;
}
/**
 * Implements the asynchronous API in terms of the synchronous API.
 */
export declare class SynchronousFileSystem extends BaseFileSystem {
    get metadata(): FileSystemMetadata;
    access(p: string, mode: number, cred: Cred): Promise<void>;
    rename(oldPath: string, newPath: string, cred: Cred): Promise<void>;
    stat(p: string | null, cred: Cred): Promise<Stats>;
    open(p: string, flags: FileFlag, mode: number, cred: Cred): Promise<File>;
    unlink(p: string, cred: Cred): Promise<void>;
    rmdir(p: string, cred: Cred): Promise<void>;
    mkdir(p: string, mode: number, cred: Cred): Promise<void>;
    readdir(p: string, cred: Cred): Promise<string[]>;
    chmod(p: string, mode: number, cred: Cred): Promise<void>;
    chown(p: string, new_uid: number, new_gid: number, cred: Cred): Promise<void>;
    utimes(p: string, atime: Date, mtime: Date, cred: Cred): Promise<void>;
    link(srcpath: string, dstpath: string, cred: Cred): Promise<void>;
    symlink(srcpath: string, dstpath: string, type: string, cred: Cred): Promise<void>;
    readlink(p: string, cred: Cred): Promise<string>;
}
