import terser from '@rollup/plugin-terser';

const base = {
  input: 'src/entry.js',
};

export default [
  {
    ...base,
    output: {
      file: 'dist/es/bundle.mjs',
      format: 'es',
    },
  },
  {
    ...base,
    output: {
      file: 'dist/es-min/bundle.mjs',
      format: 'es',
    },
    plugins: [terser()],
  },
];
