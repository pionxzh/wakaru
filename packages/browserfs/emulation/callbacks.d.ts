/// <reference types="node" />
/// <reference types="node" />
import type { FSWatcher, ReadStream, WriteStream, symlink as _symlink } from 'fs';
import { BFSCallback, BFSOneArgCallback, BFSThreeArgCallback, FileContents } from '../filesystem';
import { Stats } from '../stats';
/**
 * Asynchronous rename. No arguments other than a possible exception are given
 * to the completion callback.
 * @param oldPath
 * @param newPath
 * @param callback
 */
export declare function rename(oldPath: string, newPath: string, cb?: BFSOneArgCallback): void;
/**
 * Test whether or not the given path exists by checking with the file system.
 * Then call the callback argument with either true or false.
 * @example Sample invocation
 *   fs.exists('/etc/passwd', function (exists) {
 *     util.debug(exists ? "it's there" : "no passwd!");
 *   });
 * @param path
 * @param callback
 */
export declare function exists(path: string, cb?: (exists: boolean) => unknown): void;
/**
 * Asynchronous `stat`.
 * @param path
 * @param callback
 */
export declare function stat(path: string, cb?: BFSCallback<Stats>): void;
/**
 * Asynchronous `lstat`.
 * `lstat()` is identical to `stat()`, except that if path is a symbolic link,
 * then the link itself is stat-ed, not the file that it refers to.
 * @param path
 * @param callback
 */
export declare function lstat(path: string, cb?: BFSCallback<Stats>): void;
/**
 * Asynchronous `truncate`.
 * @param path
 * @param len
 * @param callback
 */
export declare function truncate(path: string, cb?: BFSOneArgCallback): void;
export declare function truncate(path: string, len: number, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous `unlink`.
 * @param path
 * @param callback
 */
export declare function unlink(path: string, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous file open.
 * Exclusive mode ensures that path is newly created.
 *
 * `flags` can be:
 *
 * * `'r'` - Open file for reading. An exception occurs if the file does not exist.
 * * `'r+'` - Open file for reading and writing. An exception occurs if the file does not exist.
 * * `'rs'` - Open file for reading in synchronous mode. Instructs the filesystem to not cache writes.
 * * `'rs+'` - Open file for reading and writing, and opens the file in synchronous mode.
 * * `'w'` - Open file for writing. The file is created (if it does not exist) or truncated (if it exists).
 * * `'wx'` - Like 'w' but opens the file in exclusive mode.
 * * `'w+'` - Open file for reading and writing. The file is created (if it does not exist) or truncated (if it exists).
 * * `'wx+'` - Like 'w+' but opens the file in exclusive mode.
 * * `'a'` - Open file for appending. The file is created if it does not exist.
 * * `'ax'` - Like 'a' but opens the file in exclusive mode.
 * * `'a+'` - Open file for reading and appending. The file is created if it does not exist.
 * * `'ax+'` - Like 'a+' but opens the file in exclusive mode.
 *
 * @see http://www.manpagez.com/man/2/open/
 * @param path
 * @param flags
 * @param mode defaults to `0644`
 * @param callback
 */
export declare function open(path: string, flag: string, cb?: BFSCallback<number>): void;
export declare function open(path: string, flag: string, mode: number | string, cb?: BFSCallback<number>): void;
/**
 * Asynchronously reads the entire contents of a file.
 * @example Usage example
 *   fs.readFile('/etc/passwd', function (err, data) {
 *     if (err) throw err;
 *     console.log(data);
 *   });
 * @param filename
 * @param options
 * @option options [String] encoding The string encoding for the file contents. Defaults to `null`.
 * @option options [String] flag Defaults to `'r'`.
 * @param callback If no encoding is specified, then the raw buffer is returned.
 */
export declare function readFile(filename: string, cb: BFSCallback<Buffer>): void;
export declare function readFile(filename: string, options: {
    flag?: string;
}, callback?: BFSCallback<Buffer>): void;
export declare function readFile(filename: string, options: {
    encoding: string;
    flag?: string;
}, callback?: BFSCallback<string>): void;
export declare function readFile(filename: string, encoding: string, cb: BFSCallback<string>): void;
/**
 * Asynchronously writes data to a file, replacing the file if it already
 * exists.
 *
 * The encoding option is ignored if data is a buffer.
 *
 * @example Usage example
 *   fs.writeFile('message.txt', 'Hello Node', function (err) {
 *     if (err) throw err;
 *     console.log('It\'s saved!');
 *   });
 * @param filename
 * @param data
 * @param options
 * @option options [String] encoding Defaults to `'utf8'`.
 * @option options [Number] mode Defaults to `0644`.
 * @option options [String] flag Defaults to `'w'`.
 * @param callback
 */
export declare function writeFile(filename: string, data: FileContents, cb?: BFSOneArgCallback): void;
export declare function writeFile(filename: string, data: FileContents, encoding?: string, cb?: BFSOneArgCallback): void;
export declare function writeFile(filename: string, data: FileContents, options?: {
    encoding?: string;
    mode?: string | number;
    flag?: string;
}, cb?: BFSOneArgCallback): void;
/**
 * Asynchronously append data to a file, creating the file if it not yet
 * exists.
 *
 * @example Usage example
 *   fs.appendFile('message.txt', 'data to append', function (err) {
 *     if (err) throw err;
 *     console.log('The "data to append" was appended to file!');
 *   });
 * @param filename
 * @param data
 * @param options
 * @option options [String] encoding Defaults to `'utf8'`.
 * @option options [Number] mode Defaults to `0644`.
 * @option options [String] flag Defaults to `'a'`.
 * @param callback
 */
export declare function appendFile(filename: string, data: FileContents, cb?: BFSOneArgCallback): void;
export declare function appendFile(filename: string, data: FileContents, options?: {
    encoding?: string;
    mode?: number | string;
    flag?: string;
}, cb?: BFSOneArgCallback): void;
export declare function appendFile(filename: string, data: FileContents, encoding?: string, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous `fstat`.
 * `fstat()` is identical to `stat()`, except that the file to be stat-ed is
 * specified by the file descriptor `fd`.
 * @param fd
 * @param callback
 */
export declare function fstat(fd: number, cb?: BFSCallback<Stats>): void;
/**
 * Asynchronous close.
 * @param fd
 * @param callback
 */
export declare function close(fd: number, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous ftruncate.
 * @param fd
 * @param len
 * @param callback
 */
export declare function ftruncate(fd: number, cb?: BFSOneArgCallback): void;
export declare function ftruncate(fd: number, len?: number, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous fsync.
 * @param fd
 * @param callback
 */
export declare function fsync(fd: number, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous fdatasync.
 * @param fd
 * @param callback
 */
export declare function fdatasync(fd: number, cb?: BFSOneArgCallback): void;
/**
 * Write buffer to the file specified by `fd`.
 * Note that it is unsafe to use fs.write multiple times on the same file
 * without waiting for the callback.
 * @param fd
 * @param buffer Buffer containing the data to write to
 *   the file.
 * @param offset Offset in the buffer to start reading data from.
 * @param length The amount of bytes to write to the file.
 * @param position Offset from the beginning of the file where this
 *   data should be written. If position is null, the data will be written at
 *   the current position.
 * @param callback The number specifies the number of bytes written into the file.
 */
export declare function write(fd: number, buffer: Buffer, offset: number, length: number, cb?: BFSThreeArgCallback<number, Buffer>): void;
export declare function write(fd: number, buffer: Buffer, offset: number, length: number, position: number | null, cb?: BFSThreeArgCallback<number, Buffer>): void;
export declare function write(fd: number, data: FileContents, cb?: BFSThreeArgCallback<number, string>): void;
export declare function write(fd: number, data: FileContents, position: number | null, cb?: BFSThreeArgCallback<number, string>): void;
export declare function write(fd: number, data: FileContents, position: number | null, encoding: BufferEncoding, cb?: BFSThreeArgCallback<number, string>): void;
/**
 * Read data from the file specified by `fd`.
 * @param buffer The buffer that the data will be
 *   written to.
 * @param offset The offset within the buffer where writing will
 *   start.
 * @param length An integer specifying the number of bytes to read.
 * @param position An integer specifying where to begin reading from
 *   in the file. If position is null, data will be read from the current file
 *   position.
 * @param callback The number is the number of bytes read
 */
export declare function read(fd: number, buffer: Buffer, offset: number, length: number, position?: number, cb?: BFSThreeArgCallback<number, Buffer>): void;
/**
 * Asynchronous `fchown`.
 * @param fd
 * @param uid
 * @param gid
 * @param callback
 */
export declare function fchown(fd: number, uid: number, gid: number, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous `fchmod`.
 * @param fd
 * @param mode
 * @param callback
 */
export declare function fchmod(fd: number, mode: string | number, cb: BFSOneArgCallback): void;
/**
 * Change the file timestamps of a file referenced by the supplied file
 * descriptor.
 * @param fd
 * @param atime
 * @param mtime
 * @param callback
 */
export declare function futimes(fd: number, atime: number | Date, mtime: number | Date, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous `rmdir`.
 * @param path
 * @param callback
 */
export declare function rmdir(path: string, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous `mkdir`.
 * @param path
 * @param mode defaults to `0777`
 * @param callback
 */
export declare function mkdir(path: string, mode?: any, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous `readdir`. Reads the contents of a directory.
 * The callback gets two arguments `(err, files)` where `files` is an array of
 * the names of the files in the directory excluding `'.'` and `'..'`.
 * @param path
 * @param callback
 */
export declare function readdir(path: string, cb?: BFSCallback<string[]>): void;
/**
 * Asynchronous `link`.
 * @param srcpath
 * @param dstpath
 * @param callback
 */
export declare function link(srcpath: string, dstpath: string, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous `symlink`.
 * @param srcpath
 * @param dstpath
 * @param type can be either `'dir'` or `'file'` (default is `'file'`)
 * @param callback
 */
export declare function symlink(srcpath: string, dstpath: string, cb?: BFSOneArgCallback): void;
export declare function symlink(srcpath: string, dstpath: string, type?: _symlink.Type, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous readlink.
 * @param path
 * @param callback
 */
export declare function readlink(path: string, cb?: BFSCallback<string>): void;
/**
 * Asynchronous `chown`.
 * @param path
 * @param uid
 * @param gid
 * @param callback
 */
export declare function chown(path: string, uid: number, gid: number, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous `lchown`.
 * @param path
 * @param uid
 * @param gid
 * @param callback
 */
export declare function lchown(path: string, uid: number, gid: number, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous `chmod`.
 * @param path
 * @param mode
 * @param callback
 */
export declare function chmod(path: string, mode: number | string, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous `lchmod`.
 * @param path
 * @param mode
 * @param callback
 */
export declare function lchmod(path: string, mode: number | string, cb?: BFSOneArgCallback): void;
/**
 * Change file timestamps of the file referenced by the supplied path.
 * @param path
 * @param atime
 * @param mtime
 * @param callback
 */
export declare function utimes(path: string, atime: number | Date, mtime: number | Date, cb?: BFSOneArgCallback): void;
/**
 * Change file timestamps of the file referenced by the supplied path.
 * @param path
 * @param atime
 * @param mtime
 * @param callback
 */
export declare function lutimes(path: string, atime: number | Date, mtime: number | Date, cb?: BFSOneArgCallback): void;
/**
 * Asynchronous `realpath`. The callback gets two arguments
 * `(err, resolvedPath)`. May use `process.cwd` to resolve relative paths.
 *
 * @example Usage example
 *   let cache = {'/etc':'/private/etc'};
 *   fs.realpath('/etc/passwd', cache, function (err, resolvedPath) {
 *     if (err) throw err;
 *     console.log(resolvedPath);
 *   });
 *
 * @param path
 * @param cache An object literal of mapped paths that can be used to
 *   force a specific path resolution or avoid additional `fs.stat` calls for
 *   known real paths.
 * @param callback
 */
export declare function realpath(path: string, cb?: BFSCallback<string>): void;
export declare function realpath(path: string, cache: {
    [path: string]: string;
}, cb: BFSCallback<string>): void;
/**
 * Asynchronous `access`.
 * @param path
 * @param mode
 * @param callback
 */
export declare function access(path: string, cb: BFSOneArgCallback): void;
export declare function access(path: string, mode: number, cb: BFSOneArgCallback): void;
export declare function watchFile(filename: string, listener: (curr: Stats, prev: Stats) => void): void;
export declare function watchFile(filename: string, options: {
    persistent?: boolean;
    interval?: number;
}, listener: (curr: Stats, prev: Stats) => void): void;
export declare function unwatchFile(filename: string, listener?: (curr: Stats, prev: Stats) => void): void;
export declare function watch(filename: string, listener?: (event: string, filename: string) => any): FSWatcher;
export declare function watch(filename: string, options: {
    persistent?: boolean;
}, listener?: (event: string, filename: string) => any): FSWatcher;
export declare function createReadStream(path: string, options?: {
    flags?: string;
    encoding?: string;
    fd?: number;
    mode?: number;
    autoClose?: boolean;
}): ReadStream;
export declare function createWriteStream(path: string, options?: {
    flags?: string;
    encoding?: string;
    fd?: number;
    mode?: number;
}): WriteStream;
