import { defineConfig } from 'vite';
import path from 'path';

export default defineConfig({
  build: {
    lib: {
      entry: path.resolve(import.meta.dirname, 'src/entry.js'),
      formats: ['es'],
      fileName: 'bundle',
    },
    outDir: path.resolve(import.meta.dirname, 'dist/es-min'),
    sourcemap: false,
    minify: 'esbuild',
  },
});
