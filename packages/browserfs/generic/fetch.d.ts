/// <reference types="node" />
export declare const fetchIsAvailable: boolean;
/**
 * Asynchronously download a file as a buffer or a JSON object.
 * Note that the third function signature with a non-specialized type is
 * invalid, but TypeScript requires it when you specialize string arguments to
 * constants.
 * @hidden
 */
export declare function fetchFile(p: string, type: 'buffer'): Promise<Buffer>;
export declare function fetchFile(p: string, type: 'json'): Promise<any>;
export declare function fetchFile(p: string, type: string): Promise<any>;
/**
 * Asynchronously retrieves the size of the given file in bytes.
 * @hidden
 */
export declare function fetchFileSize(p: string): Promise<number>;
