/// <reference types="node" />
import * as fs_mock from './index';
import type * as fs_node from 'node:fs';
type BrowserFSModule = typeof fs_node & typeof fs_mock;
declare const fs: BrowserFSModule;
export * from './index';
export default fs;
