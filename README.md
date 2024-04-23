# Wakaru

[![deploy][DeployBadge]][Playground]
[![codecov][CodecovBadge]][CodecovRepo]
[![Telegram-group](https://img.shields.io/badge/Telegram-group-blue)](https://t.me/wakarujs)


Wakaru is the Javascript decompiler for modern frontend. It brings back the original code from a bundled and transpiled source.

<!-- Try it out at the [Playground][Playground]. -->

- 🔪📦 Unpacks bundled JavaScript into separated modules from [webpack][webpack] and [browserify][browserify].
- ⛏️📜 Unminifies transpiled code from [Terser][Terser], [Babel][Babel], [SWC][SWC], and [TypeScript][TypeScript].
- ✨📚 Detects and restores downgraded syntaxes (even with helpers!). See the [list](./packages//unminify/README.md#syntax-upgrade).
- 🧪🛡️ All cases are protected by tests. All code is written in TypeScript.

## Demo

See [live demo][Demo] for detailed examples.

## Features

### Unminify

Converts transpiled code back to its readable form and restores downgraded syntaxes.

Supports the following transpilers:
  - Terser
  - Babel
  - SWC
  - TypeScript

[Read the documentation](./packages/unminify/README.md) for more information.

### Unpacker

Converts bundled JavaScript into separated modules

Supports the following bundlers:
  - webpack
  - browserify

## Try it out

Test the tool and see it in action at [Playground][Playground].

## 🖥 Using the CLI

### Interactive mode

By default, the CLI will run in interactive mode and guide you through the process.\
You can also pass [options](#options) to skip some steps in the interactive mode.

```sh
npx @wakaru/cli
# or
pnpm dlx @wakaru/cli
```

### Options

Run `npx @wakaru/cli --help` to see the full list of options.

| Option          | Default | Description                             |
| --------------- | ------- | --------------------------------------- |
| `--output`      | `"out"` | Output directory                        |
| `--force`       | `false` | Force overwrite output directory        |
| `--concurrency` | `1`     | Specific the number of concurrent tasks |
| `--perf`        | `false` | Show performance metrics                |
| `--perf-output` |         | Performance metrics output directory    |

`--concurrency` can be used to speed up the process. But please aware that the process might OOM if the input file is too large.

### Non-interactive mode

If you want to run the CLI in non-interactive mode, you can specify the feature by passing the feature name as the first argument.

`unpacker` and `unminify` will run only the corresponding feature.\
`all` will run both `unpacker` and `unminify` sequentially.

```
npx @wakaru/cli all      <files...> [options]
npx @wakaru/cli unpacker <files...> [options]
npx @wakaru/cli unminify <files...> [options]
```

These options are **only** available in `all` mode.

| Option              | Default          | Description                        |
| ------------------- | ---------------- | ---------------------------------- |
| `--unpacker-output` | `"out/unpack"`   | Override unpacker output directory |
| `--unminify-output` | `"out/unminify"` | Override unminify output directory |

When running a single feature (either `unpacker` or `unminify`), the CLI will only uses the path specified in the `--output` option. This means that, unlike in the `all` mode where subdirectories (`out/unpack` and `out/unminify`) are automatically created within the output directory, in single feature mode, the output files are placed directly in the specified `--output` directory without any additional subdirectories.

## 📦 Using the API

```sh
npm install @wakaru/unpacker @wakaru/unminify
# or
pnpm install @wakaru/unpacker @wakaru/unminify
# or
yarn add @wakaru/unpacker @wakaru/unminify
```

<details>

<summary>Click to expand</summary>

### `@wakaru/unpacker`

```ts
import { unpack } from '@wakaru/unpacker';

const { modules, moduleIdMapping } = await unpack(sourceCode);
for (const mod of modules) {
  const filename = moduleIdMapping[mod.id] ?? `module-${mod.id}.js`;
  fs.writeFileSync(outputPath, mod.code, 'utf-8');
}
```

### `@wakaru/unminify`

```ts
import { runDefaultTransformationRules, runTransformationRules } from '@wakaru/unminify';

const file = {
  source: '...', // source code
  path: '...',   // path to the file, used for advanced usecases. Can be empty.
}
// This function will apply all rules that are enabled by default.
const { code } = await runDefaultTransformationRules(file);

// You can also specify the rules to apply. Order matters.
const rules = [
  'un-esm',
  ...
]
const { code } = await runTransformationRules(file, rules);
```

You can check all the rules at [/unminify/src/transformations/index.ts](https://github.com/pionxzh/wakaru/blob/main/packages/unminify/src/transformations/index.ts).

Please aware that this project is still in early development. The API might change in the future.

And the bundle size of these packages are huge. It might be reduced in the future. Use with caution on the browser (Yes, like the playground, it can run on the browser ✨).

</details>

## Legal Disclaimer

Usage of `wakaru` for attacking targets without prior mutual consent is illegal. It is the end user's responsibility to obey all applicable local, state and federal laws. Developers assume no liability and are not responsible for any misuse or damage caused by this program.

[TypeScript]: https://www.typescriptlang.org/
[browserify]: http://browserify.org/
[webpack]: https://webpack.js.org/
[Terser]: https://terser.org/
[Babel]: https://babeljs.io/
[SWC]: https://swc.rs/
[Playground]: https://wakaru.vercel.app/

[CodecovBadge]: https://img.shields.io/codecov/c/github/pionxzh/wakaru
[CodecovRepo]: https://codecov.io/gh/pionxzh/wakaru

[Demo]: https://wakaru.vercel.app/#eNq9WG1z00YQ/iuHPnTswXHkl/gllLZAodACyYBbBuIMI0sn+4IsiZPkOGTy3/vs6WSfZTuJwa0+JNrn9vZ29/ZNvrbcyOPWsTW0soSzJJXCTYfWo2E4cyST7DGT/GsmJK90251mVeORgXd6R23Ch+HhIfud+04WpCx2pDPlKZfJMPSz0E1FFLKLSpVdD0OGh4Q4EOLIcTblYZrUAx6O0wn7hdnsp5+W+Jl9zh48fsyy0OO+CLnHfl1dPGZD61XoR0rnQvRos+jGyuYGbV7I1bthw4BP48BJOQsEDHCCfMEJuEwrQ+tsaLGH0P0hzj1nb3iSOGPOfBlNAQAdkS9utDve8RBuYG40jaMQh7KRk8AE+CKdcOaJBAddvQVL7lYPai+8VeELb0HSyeiCuynzOG4oc9NMinC8tDfFRl53JyLwJA9rLMzpwEkSkl5jrgJmIhGjAKSvSNJNXZw+44mUztW2I2Jsqdg1JuuIk/cp/FOFN4ZWtcYmeimqf6pW4hpTYVLsG2NxgouqsSv11jhfelq7Z87SiCU8/UD//SgIokvlHyxCBXgvnMF38MnqBZe1caHKzFRlVFLlEoszpcpcvW1QRaTsUiBW3nGfJZnvi/ly+zfzTDBUK2EWBHnoaynvY8QMTPma8dDljM9jiQBZaA6OF2LOROghcXCbrhPo6JIc/sadL+Q/931wwMVqmZ5lYCzionhIbiBiFWeOFAmYHrKnURRwJ8zdJ5dKFA+ZxGHSA5shv1ykxtCCm7lEGFNSYfOMq6xaO+xZBAtIl4RuLIjGApasWVs8LiX0g0v6O688sJcOM9WPJOREMcm7RBgj+UCtsvngqTxigv3MGvajKhmWwMQ6zq+Idalk32eK79JRJzFpDn3diSPCRYwXj/BxC5/JMcg/qjx0yaQ7MCJnkfDyGpVzfeZ1JxRTh4TesQGsiDIkoLd2g4XClLjIhIGY8ihLK7fcuWHR5oJVfq5UutbhNdeB5Bqu+ADpWwCoCrh4A9jMceBAIYqLapFZ5cdF4MjCgHAT102NNXhr/caMRLiH4fc4p4ShKi9VWL6fuTV2eZ7TVY3Luis5XPo84NQs4DpPzIZWjV3jXF1VKUmWDkXVXfXfGOVIcv8YdQMGp8pf0GBTV0DQP0uSgXTCRKXVMPTqRnNATAytVQZKS6tmTZ04RgRbx9dWs40mPo28LOAHzXb9IsFyv7HE+o0ca/QMEESOthrGdhA52m7ZSxREjnZbhgQQBdo00abWwO4aKthdrYNtG8xEabzZ6hg4KI13e0cGDqqwpWWoTVSON+22cSxRGm/0+gYOSltvHxk4URpvH5luAaXxfsuQT5T211GjZzgMlMY7rZaBg8rxI7tr4ERpvHVkOJ4ojfc7hj5EFXjX5AeV451ux7gqojTeaxv8RBW4aS9R+m5t029EaRwjoYGD0ninY9hFlMa7bVMOKB0lnb5x70RpvGcbehKl8b7pH6IIv0E+8NQpkuHaEsgvmVrHZ9dWehXTkItZj8ZTiKAMBCDxmkSZdEF0m/32zXnNQh9T2zAah2AZgeWvE7zM6ITUGSe0NIJY8M7o3w1wSjXjxBUxn9RBxmYJDuu3kTPiwaHMMNpM+eGEBzHa9GGkBr0PGEJQ105lBDAVPLHUKSp5dzYMebdqFykUmQpFtyiUF0I06BP/FbWXNJIv1RJOOGBbNmVhksV0HPfUJjS9QaTmS+zassehZW34SzR2uumtByjm1+LLQqzyjypju188AnfdQbHpoOB/dBA0yW45bg9uUnV9a7QOVHTUrIu/tZ9iSk5huINkqC7wH7g6l42ysrNsqlfrsst6q460l0zdw0XkjXAv2pCHIl+LVX1098tBQS458BQs+LPqwrwd76G+vgCHA463G+4p7+07H0LNez0IPpWE5wPCHiwYkIMoWUbUCAVevnxQRpXOU4PHHs57o09BgVip4Kr4qSlmHylJRk3K0tUstCVUS6xqPNpZEQqru68un7HupUc+du2sB81Vd+uRz267G4mMv7tI5QPgHirgxYZgzKfI74uT2nZe3+RtNW5jjU1vw4/rDnm+pjNNuNsKJYaxlUJJO/LZdw8pd0ojsQuWAYY9DAI16x/6GPHw8hEFWdk9+lPn5cUfeKFGoJpnhpf5M13kvtH2cs7mk/i9ojkfzncPuF7jB4a/JBAu95aj2+39jZrhnZ1Q/UaxHHdei6kg1bfyh1FYTEjv8NPk9w5Vaq5Q3zHbosi/3DSKqE+cvXRoPk956OlJPv9EutfF519Ne4jk9x91KA5oxKNLivP2VZqQ1NfYj/eRklT1Lbd7Me71W7eVEuq9Kxbfwntq8GIEXnVOwX1sPQUbfn6e8idpNAX9quwf9fW5p/HUXRF+c/MvVs7HqQ==
[DeployBadge]: https://therealsujitk-vercel-badge.vercel.app/?app=wakaru

## License

[MIT](./LICENSE)
