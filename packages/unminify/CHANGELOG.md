# Changelog

## [0.2.1](https://github.com/pionxzh/wakaru/compare/unminify-v0.2.0...unminify-v0.2.1) (2024-04-23)


### Features

* **un-sequence-expression:** support for in and for of loop ([f584dc1](https://github.com/pionxzh/wakaru/commit/f584dc1254a145a5f8242829419f305aaeb6d685))
* **unminify:** add `un-import-rename` rule ([95d3d00](https://github.com/pionxzh/wakaru/commit/95d3d005a308b2c702d496bcc42d541b2930f77a))


### Bug Fixes

* improve removing temporary variables ([288f71b](https://github.com/pionxzh/wakaru/commit/288f71b9226a1f3836693314bec50f72aefe1296))
* **un-default-parameter:** handle parameters with gap ([#124](https://github.com/pionxzh/wakaru/issues/124)) ([1458630](https://github.com/pionxzh/wakaru/commit/1458630e9b676be0f2deb15c57c4ebc296531567))
* **un-indirect-call:** better naming conflicts handling ([976d80f](https://github.com/pionxzh/wakaru/commit/976d80f2089a480e5c6f68c3e8fb485bc04106e7))
* **un-optional-chaining:** handle edge case when optional chaining are concated ([3c8190f](https://github.com/pionxzh/wakaru/commit/3c8190f908200c2bc927de7d890be076d6082dbb))
* **un-sequence-expression:** keep the last assignment in for init ([68e36c5](https://github.com/pionxzh/wakaru/commit/68e36c58f819670c4b9cc28940666310282e6e9b))
* **un-sequence-expression:** shouold not split seqs in while condition ([4607903](https://github.com/pionxzh/wakaru/commit/4607903c2bf1a3fc639d466c4f2eabbee0095b08)), closes [#122](https://github.com/pionxzh/wakaru/issues/122)

## [0.2.0](https://github.com/pionxzh/wakaru/compare/unminify-v0.1.5...unminify-v0.2.0) (2024-03-02)


### ⚠ BREAKING CHANGES

* update the format of conflic name from `{name}$0` to `{name}_1`

### Features

* add `un-argument-spread` into `un-parameters` ([92fec7f](https://github.com/pionxzh/wakaru/commit/92fec7fdc48dfc4297a40ee96d03a894dd2d440a))
* add `un-argument-spread` rule ([1f5fbd6](https://github.com/pionxzh/wakaru/commit/1f5fbd63876a32cd87d48c7b032af9b6440f7d28))
* **smart-inline:** support global variable inlining and property access path renaming ([1a91aa4](https://github.com/pionxzh/wakaru/commit/1a91aa4418e8e606efa6a4a6b3e3b4966195a38f))


### Bug Fixes

* adjust iife rule order ([b5c3fc7](https://github.com/pionxzh/wakaru/commit/b5c3fc74edbcba7540de8fb4959bee29f7e8f623))
* **smart-rename:** remove destructuring length limitation ([23cf3e4](https://github.com/pionxzh/wakaru/commit/23cf3e45358513e59d60dfd4370ff3881bf84f9d))
* update the format of conflic name from `{name}$0` to `{name}_1` ([dac2125](https://github.com/pionxzh/wakaru/commit/dac212547e9a1b941a856d6ff317dcc976584184))

## [0.1.5](https://github.com/pionxzh/wakaru/compare/unminify-v0.1.4...unminify-v0.1.5) (2024-02-16)


### Bug Fixes

* **jsx:** should not transform `document.createElement` to JSX ([86469e7](https://github.com/pionxzh/wakaru/commit/86469e73dbc85cbb0919552b29bb92c54fe996c6))
* **smart-inline:** fix missing renaming in property descturing ([55e2938](https://github.com/pionxzh/wakaru/commit/55e293808c98aa91e38acd706559a0d561b44764)), closes [#117](https://github.com/pionxzh/wakaru/issues/117)
* **un-esm:** fix missing bare import ([7f6f199](https://github.com/pionxzh/wakaru/commit/7f6f1995146944c1fb050714eef07c402fa293a4))
* **un-indirect-call:** should not remove unused default imports from unprocessed sources ([cfc00a9](https://github.com/pionxzh/wakaru/commit/cfc00a9f812d591e529eedc4113c03230327766a))
* **un-jsx:** disable preact's `h` pragma to avoid conflicting with normal minified code ([a97c8ea](https://github.com/pionxzh/wakaru/commit/a97c8eaff917236e297df21f2300530351fa5b11))

## [0.1.4](https://github.com/pionxzh/wakaru/compare/unminify-v0.1.3...unminify-v0.1.4) (2024-01-21)


### Features

* support async transformation ([9e37839](https://github.com/pionxzh/wakaru/commit/9e37839a731f492cf7719f4a66e8feced975fc66))


### Bug Fixes

* bump `lebab` to 3.2.4 ([fb1df55](https://github.com/pionxzh/wakaru/commit/fb1df556a3af22d77ff0e7d1a75ff9480d638e1a))

## [0.1.3](https://github.com/pionxzh/wakaru/compare/unminify-v0.1.2...unminify-v0.1.3) (2024-01-09)


### Bug Fixes

* handle escaped character in template literal ([ed66bf6](https://github.com/pionxzh/wakaru/commit/ed66bf647004f16955e11ba5fe3a0330ea9260fa))
* print filename on warning "Multiple exports of ..." ([d784322](https://github.com/pionxzh/wakaru/commit/d784322248924ec94c96609b0949bfd2f82eecd1))
* should stale the scope after renaming or adding new varaible ([3afdd7c](https://github.com/pionxzh/wakaru/commit/3afdd7cadcf75af5fb8f4ff0dbf5e87a80165771))
* **un-esm:** properly handle the export order of mixed exports ([90b6c47](https://github.com/pionxzh/wakaru/commit/90b6c479370f7b8a1ab25200494399715ca3d272))
* **unminify:** add error handling for code printing ([4e2b952](https://github.com/pionxzh/wakaru/commit/4e2b9525984312df7831131038aa9495515eb84c))
* **unminify:** add error handling to early exit the rule loop ([22bd4c0](https://github.com/pionxzh/wakaru/commit/22bd4c0e3f0b1169a50191d68441f07ef2d19626))

## [0.1.2](https://github.com/pionxzh/wakaru/compare/unminify-v0.1.1...unminify-v0.1.2) (2024-01-06)


### Bug Fixes

* fix errors from `fromPaths` ([8573cd8](https://github.com/pionxzh/wakaru/commit/8573cd83ceea9440d802d0670650737e990b2862))

## [0.1.1](https://github.com/pionxzh/wakaru/compare/unminify-v0.1.0...unminify-v0.1.1) (2023-12-30)


### Features

* support dynamic jsx tag ([0b51e23](https://github.com/pionxzh/wakaru/commit/0b51e232f32f4cf5b4a739dcc9391457d998b32a))
* **un-esm:** add annotation for requires with missing module ([2999909](https://github.com/pionxzh/wakaru/commit/29999096f701de6531eacae68d8f44f0cf05d2b0)), closes [#77](https://github.com/pionxzh/wakaru/issues/77)

## [0.1.0](https://github.com/pionxzh/wakaru/compare/unminify-v0.0.6...unminify-v0.1.0) (2023-12-15)


### ⚠ BREAKING CHANGES

* bump Node.js to >= 18
* deprecate `@wakaru/unminify` and `@wakaru/unpacker` CLI in favor of `@wakaru/cli`

### Features

* bump Node.js to &gt;= 18 ([c36d0a0](https://github.com/pionxzh/wakaru/commit/c36d0a0176db56e98841051db264ab4c4f13739d))
* **cli:** implement new standalone CLI with better UX ([deda1df](https://github.com/pionxzh/wakaru/commit/deda1df1c2894c7e9b2b443c01033d366eec549c))
* deprecate `@wakaru/unminify` and `@wakaru/unpacker` CLI in favor of `@wakaru/cli` ([be2012e](https://github.com/pionxzh/wakaru/commit/be2012e112145e0025cef7aa74a9686c0f952a6d))


### Bug Fixes

* improve error log ([df455eb](https://github.com/pionxzh/wakaru/commit/df455eb5fc4186d0d57d7ae5d676a8b45407ad64))
* **smart-rename:** handle invalid/reserved identifier ([2821b6d](https://github.com/pionxzh/wakaru/commit/2821b6d416b9393094554cae0f78eb155351c8b7))
* **un-esm:** skip invalid import name ([8182f99](https://github.com/pionxzh/wakaru/commit/8182f99e0f6c7d0013a6062755bd4880fc74a445))

## [0.0.6](https://github.com/pionxzh/wakaru/compare/unminify-v0.0.5...unminify-v0.0.6) (2023-12-04)


### Features

* **un-assignment-merging:** add new rule `un-assignment-merging` for spliting chained assignment ([59e2929](https://github.com/pionxzh/wakaru/commit/59e29290918840e6c7d373644715017384aabfb2))
* use `zod` to validate transformation param ([57407d1](https://github.com/pionxzh/wakaru/commit/57407d1e2b749b4475cca1760c40a861a52c8440))
* use array based rules `transformationRules` ([b67da89](https://github.com/pionxzh/wakaru/commit/b67da89ef922667dc05ea18f7a866917e49ab8e4))


### Bug Fixes

* **cli:** paths out of cwd is not allowed ([7cee0c8](https://github.com/pionxzh/wakaru/commit/7cee0c8d461a12fb710a44722be043065cf072ed))
* **cli:** use common base dir as the base of output folder ([7a939b7](https://github.com/pionxzh/wakaru/commit/7a939b78f975b229d0efbc3088e149970d0d6626)), closes [#47](https://github.com/pionxzh/wakaru/issues/47)
* **playground:** refactor and improve rule list ([08295e5](https://github.com/pionxzh/wakaru/commit/08295e599f73ac8cd906df73a5d7c123149db778))

## [0.0.5](https://github.com/pionxzh/wakaru/compare/unminify-v0.0.4...unminify-v0.0.5) (2023-11-26)


### Features

* better support for runtime: automatic ([237403f](https://github.com/pionxzh/wakaru/commit/237403f4d39ae949eb1c7b21daaa121b18b187de)), closes [#46](https://github.com/pionxzh/wakaru/issues/46)
* **esmodule-flag:** support various `__esModule` flag ([397d5b3](https://github.com/pionxzh/wakaru/commit/397d5b3696d1f4e38dec0ea3ecf007a03cc2b0c9))
* **smart-inline:** support computed property destructuring ([0e4bdd9](https://github.com/pionxzh/wakaru/commit/0e4bdd93ca76f91704cc884647da78268a01e0dd))
* **smart-rename:** support `forwardRef` ([d9db525](https://github.com/pionxzh/wakaru/commit/d9db5253e53f8339751cf4a8167d465177cb787e)), closes [#48](https://github.com/pionxzh/wakaru/issues/48)
* **smart-rename:** support `useReducer` ([381a2fd](https://github.com/pionxzh/wakaru/commit/381a2fdc8661627d9a46633fde8474735ec66608))
* **un-es6-class:** support detecting imported helpers ([c44f01a](https://github.com/pionxzh/wakaru/commit/c44f01a807210a5af09249b6994eec96d6f8fff3))
* **un-es6-class:** support extending super class ([19b35b9](https://github.com/pionxzh/wakaru/commit/19b35b971f68948aca338dc7a21e3a08e6581d84))


### Bug Fixes

* **smart-inline:** should not remove lonely property access ([8a7d455](https://github.com/pionxzh/wakaru/commit/8a7d455f5e8a1b212e88272ebcce45c006761fff))
* **smart-rename:** handle naming conflic with top-down transform ([01d2ac9](https://github.com/pionxzh/wakaru/commit/01d2ac977ceb4d5d397fbee880bdb67cd0e443ef)), closes [#54](https://github.com/pionxzh/wakaru/issues/54)
* supress multiple exports warning for code generated by babel ([ba3a6ba](https://github.com/pionxzh/wakaru/commit/ba3a6ba777f29bd29341501ceffb5dba7dcfeec5)), closes [#53](https://github.com/pionxzh/wakaru/issues/53)
* **un-esm:** improve multiple export and mixed export ([0f120ca](https://github.com/pionxzh/wakaru/commit/0f120cabb9a1617bd544423c3cb89a1b149ebb52))
* **un-esm:** support `module.exports.default = module.exports` ([5a21401](https://github.com/pionxzh/wakaru/commit/5a214011d8eea0b0ed5f93ba241b490ca7eeff24)), closes [#63](https://github.com/pionxzh/wakaru/issues/63)

## [0.0.4](https://github.com/pionxzh/wakaru/compare/unminify-v0.0.3...unminify-v0.0.4) (2023-11-22)


### Features

* **un-es6-class:** support methods in Babel loose mode ([4f9d9fc](https://github.com/pionxzh/wakaru/commit/4f9d9fc194c8e195f42365e9a86e6b9ec4ca90b2))


### Bug Fixes

* **enum:** should check and handle invalid identifier ([67867f7](https://github.com/pionxzh/wakaru/commit/67867f736a7bc37d7e6ed7f7b925cd8a2e93d48a)), closes [#52](https://github.com/pionxzh/wakaru/issues/52)
* **smart-rename:** stale the scope after renaming ([4bbd8a0](https://github.com/pionxzh/wakaru/commit/4bbd8a04e179ea8da2cad4abc1aa34d7f73dca24)), closes [#54](https://github.com/pionxzh/wakaru/issues/54)

## [0.0.3](https://github.com/pionxzh/wakaru/compare/unminify-v0.0.2...unminify-v0.0.3) (2023-11-19)


### Bug Fixes

* avoid removing output folder ([02f8d46](https://github.com/pionxzh/wakaru/commit/02f8d4631dcbe62f2a91b6b0b88811dd06d31039))
* move internal packages to devDeps ([da665a0](https://github.com/pionxzh/wakaru/commit/da665a09d4e2915fcc8d80e6f687c723160bf097))

## [0.0.2](https://github.com/pionxzh/wakaru/compare/unminify-v0.0.1...unminify-v0.0.2) (2023-11-19)


### Features

* `un-async-await` for restoring __generator helper ([eeb18da](https://github.com/pionxzh/wakaru/commit/eeb18dac4790ffcad38e68f4e7ad822444ae279e))
* `un-bracket-notation` support float number ([5b2f67a](https://github.com/pionxzh/wakaru/commit/5b2f67ab4c6acd67318a65c2f08024699ede9f89))
* `un-es-helper` support __esModule` flag from ES3 ([e9414d9](https://github.com/pionxzh/wakaru/commit/e9414d93adb99db4d9809beae4ef4c22e650fe4d))
* `un-esm` import ([ba5985f](https://github.com/pionxzh/wakaru/commit/ba5985f574482d223b2a98b191108f9cc31164ad))
* add `un-block-statement` ([dd38267](https://github.com/pionxzh/wakaru/commit/dd38267a619a38e43704397b178158f463554d93))
* add `un-builtin-prototype` ([3614fe2](https://github.com/pionxzh/wakaru/commit/3614fe25e37e9ea510d5126b6438417c5bb5377b))
* add `un-enum` rules for ts enum ([d661d25](https://github.com/pionxzh/wakaru/commit/d661d2559d90e28586895a1e101763da9e84a0b6))
* add `un-iife` ([fba8056](https://github.com/pionxzh/wakaru/commit/fba805626f0f16b15c83cc333b77a46fbd3ee67c))
* add `un-nullish-coalescing` ([0a7c5ce](https://github.com/pionxzh/wakaru/commit/0a7c5ce38d7b658d4edacdeb8b9578c4f36fb667))
* add `un-optional-chaining` ([6f3e387](https://github.com/pionxzh/wakaru/commit/6f3e38712cf3b13c37e3bb79544d299c0a086dbe))
* add `un-return` ([8acbe02](https://github.com/pionxzh/wakaru/commit/8acbe029d38651779f8865f28fcbccf34a053e22))
* add `un-type-constructor` ([1eb89d7](https://github.com/pionxzh/wakaru/commit/1eb89d741fc310f3e7f4dd3f4bf13fa49b7a0691))
* add `un-while` ([fc95e03](https://github.com/pionxzh/wakaru/commit/fc95e03b7ae5b0b826457a38054da16288d7918a))
* add comments to number literal ([b65a4ee](https://github.com/pionxzh/wakaru/commit/b65a4ee62e3ecde10b0c1d948ed8e74ce3b4378d))
* add rule `un-bracket-notation` ([ddd5dd8](https://github.com/pionxzh/wakaru/commit/ddd5dd8402d2e46cd20dc6c109374a7d99e63b17))
* add rule `un-infinity` ([c6e68a7](https://github.com/pionxzh/wakaru/commit/c6e68a7e66c0c5e81d7e2d360667dfda702a41dc))
* babel runtime scan ([a2587de](https://github.com/pionxzh/wakaru/commit/a2587ded911cc61ca31c893851e7c4417015658b))
* **babel-helpers:** add `arrayLikeToArray` and `arrayWithoutHoles` ([eddd01b](https://github.com/pionxzh/wakaru/commit/eddd01be6d6694287dbe9529e248dbce8cb3cb7f))
* **babel-helpers:** add `objectSpread` ([6000c0f](https://github.com/pionxzh/wakaru/commit/6000c0f45ad7a86dff4cdfd46414c920a191e81a))
* **babel-helpers:** support `extends` and improve `objectDestructuringEmpty` ([c1957b6](https://github.com/pionxzh/wakaru/commit/c1957b63819f416237246bf6d53f19072ce93536))
* **babel-helpers:** support `slicedToArray` ([194c808](https://github.com/pionxzh/wakaru/commit/194c80868cff6b1c27272d38b4f0eb311b0b54de))
* **babel-runtime:** support `interopRequireDefault` ([55b64fa](https://github.com/pionxzh/wakaru/commit/55b64fa2dcb57183476acb64b50cbcce40894a55))
* **babel-runtime:** support `interopRequireWildcard` ([bc0006c](https://github.com/pionxzh/wakaru/commit/bc0006ccf378859ed5b6fbc174947c3eb4ebffa2))
* build package ([2c281bc](https://github.com/pionxzh/wakaru/commit/2c281bc29af5e609bbd352b1bde4b14d5b3122e5))
* **createForOfIteratorHelper:** support for more cases ([85aff28](https://github.com/pionxzh/wakaru/commit/85aff2888a314b6cbbce51dd04fef0e90edf5215))
* drop `5to6-codemod`, use `un-esm` instead ([327dc2e](https://github.com/pionxzh/wakaru/commit/327dc2e493fb80d9fd1ad38e4d34d70a0efab7b9))
* **esm:** support namespace import and dynamic import ([aa26d8e](https://github.com/pionxzh/wakaru/commit/aa26d8e5a62616cfcd9df4e8b98ab1e839b40741))
* expand coverage for `un-while-loop` ([66f19d3](https://github.com/pionxzh/wakaru/commit/66f19d309a076f60a218fe7b0cea18e53f255207))
* extract `@unminify/ast-utils` ([9c5fe11](https://github.com/pionxzh/wakaru/commit/9c5fe1147001edf1620fb3e1660010ee81ba2408))
* **flip-operator:** support `void 0`, MemberExpression and CallExpression ([36d84bf](https://github.com/pionxzh/wakaru/commit/36d84bf1122c0eae4e05b9bfb04da7c8e800080f))
* **iife:** detect various iife ([0356085](https://github.com/pionxzh/wakaru/commit/035608500def05031f24dc2c46e224eda986cac4))
* implement return switch ([13ec5a6](https://github.com/pionxzh/wakaru/commit/13ec5a6801183e43af8af68a77c5e19ca54e9a5e))
* implement template literal upgration ([e507180](https://github.com/pionxzh/wakaru/commit/e5071803532ff13a1c75717370d184d918898a8a))
* improve cjs to esm transformation ([4a1ec75](https://github.com/pionxzh/wakaru/commit/4a1ec7592cb94f6f791dda12ab7efeab67462103))
* improve comment retention ([266871a](https://github.com/pionxzh/wakaru/commit/266871ad949d40c0d6bd3bbb586706f65de45787))
* improve coverage of `un-sequence-expression` ([99c66ed](https://github.com/pionxzh/wakaru/commit/99c66ed9075b12f1d69cf527258e5a5c1804f834))
* improve flip operator ([330461a](https://github.com/pionxzh/wakaru/commit/330461a7ea96858ed37c1ca866f16b15560e16d4))
* **lebab:** enable lebab's class rule ([e9e160f](https://github.com/pionxzh/wakaru/commit/e9e160fd8d00f7e2db8689126a340e4557c76c33))
* merge swtich transformation into un-if-statement` ([0e04d01](https://github.com/pionxzh/wakaru/commit/0e04d0146520553dfb2e565a01a968f5b81751d2))
* **parameter:** support arrow function and class method ([b7835e9](https://github.com/pionxzh/wakaru/commit/b7835e97b044a43deabf046522be6873626c499f))
* **parameter:** support parameter with logic minifier ([7fd3fb5](https://github.com/pionxzh/wakaru/commit/7fd3fb5188055a0d94c903231d73dfa2126b6c56))
* playground ([74075f4](https://github.com/pionxzh/wakaru/commit/74075f43e8c47aabe23fe6bd680fc57a31ede219))
* pre scan module meta ([70fde64](https://github.com/pionxzh/wakaru/commit/70fde646079d483dc4b91fff2d206c3ef786adc8))
* remove temp varaible ([b20b705](https://github.com/pionxzh/wakaru/commit/b20b70554024e535452ea2e467677545558e060a))
* separate variable declarators in for statement ([d48d382](https://github.com/pionxzh/wakaru/commit/d48d382846b5b7a681d83b1a07ddb6f5d9dc399b))
* **seq:** improve seq expr output for assignment and support arrow function body split ([a5ac21e](https://github.com/pionxzh/wakaru/commit/a5ac21e49180d469123c2b49b99989eeae720f09))
* **seq:** split assignment inside assignment ([3bac009](https://github.com/pionxzh/wakaru/commit/3bac00980137190e53a50b52edaef8ef7ab93750))
* **smart-inline:** implement destructuring ([42eea37](https://github.com/pionxzh/wakaru/commit/42eea37f0e04f05154a6403304ffaa3ff37483dc))
* **smart-inline:** support inlining temp variable ([a5b978d](https://github.com/pionxzh/wakaru/commit/a5b978d060ec48a0786d363d3719be9908016aa6))
* **smart-rename:** add rule `smart-rename` ([3c5d8be](https://github.com/pionxzh/wakaru/commit/3c5d8be3f27c4fe61a156c3cacdfa37ab7fec6ea))
* **smart-rename:** support function param descturing ([90ff587](https://github.com/pionxzh/wakaru/commit/90ff587962c6da4b827ffeecf003df51695b09de))
* **smart-rename:** support renaming based on react API ([f35d07d](https://github.com/pionxzh/wakaru/commit/f35d07d5742d127de1366cf13918c511ff60bb1f))
* support minified enum ([f7b882e](https://github.com/pionxzh/wakaru/commit/f7b882e0f6e1a5b8223a85d5f159ba5c419a3fcf))
* support restoring  `__await ` to async await ([7550cd7](https://github.com/pionxzh/wakaru/commit/7550cd76f2c5235136045b8436de9eeb072b0400))
* **toConsumableArray:** add babel `toConsumableArray` helper ([47e6186](https://github.com/pionxzh/wakaru/commit/47e6186066708fda6b43971b47eb1ffe1fcfecce))
* transform numeric literal ([aff4033](https://github.com/pionxzh/wakaru/commit/aff40338d707fdec8db2858f96b0b74a29f931c6))
* **typeof:** support `typeof x &gt; "u"` ([8bcbe6a](https://github.com/pionxzh/wakaru/commit/8bcbe6a41b7f5f42711dd32ecd84b69e71960a34))
* **un-indirect-call:** implement indrect call replacement ([2594302](https://github.com/pionxzh/wakaru/commit/25943028817dfcf99c424afd867fbd4ffc246d84))
* **un-jsx:** support jsx transformation ([2e7a0fa](https://github.com/pionxzh/wakaru/commit/2e7a0fa9249b833d66fc44c5b9579e325e54b26f))
* **un-jsx:** support rename component based on `displayName` ([8bda66e](https://github.com/pionxzh/wakaru/commit/8bda66e21b86fb9fbc88ba71c02b95bc2442736b))
* **un-parameter:** add support for default and non-default parameter, rest not yet ([17ac4d7](https://github.com/pionxzh/wakaru/commit/17ac4d7d6c45878eebab1aa96a02eb564b6e39b6))
* **un-parameter:** correctly check parameter usage ([fdaa584](https://github.com/pionxzh/wakaru/commit/fdaa5842fe04bceb3d0770d22c378844219d51c3))
* **un-variable-merging:** split export varaible declaration ([e7035eb](https://github.com/pionxzh/wakaru/commit/e7035eb0c5f0207f8b23f68dc8bf11784aeeb81c))
* **unminify:** implement cli ([16935d5](https://github.com/pionxzh/wakaru/commit/16935d5989d83ee17cce34db6d856b2717f2cad5))


### Bug Fixes

* `jsxs` should be listed as pragma for react runtime ([aa292e9](https://github.com/pionxzh/wakaru/commit/aa292e923ce6f3bc21e6b2d94ffa81a8d94c5a9c))
* `un-variable-merging` filter ([22e9c13](https://github.com/pionxzh/wakaru/commit/22e9c1330025817618cbf90caefb9e3cef3fb7d2))
* `un-variable-merging` should respect parent scope binding ([d3df7b5](https://github.com/pionxzh/wakaru/commit/d3df7b55a4a2f8ec76b66091f31d9621a7f9bfc2))
* add `un-sequence-expression` after `un-conditionals` ([9921c5c](https://github.com/pionxzh/wakaru/commit/9921c5cdbd7f3e9acb9edbf784caedc46679ae7e))
* add createForOfIteratorHelper to the list ([25a7d4c](https://github.com/pionxzh/wakaru/commit/25a7d4c52911541d693f094a83184cfd7fb565e5))
* adjust rule order to improve curly braces and seq spliting ([48f9312](https://github.com/pionxzh/wakaru/commit/48f931252f256a41aa7391e521cf596bcffc7553))
* adot `transformToMultiStatementContext` for better replacement ([40702d5](https://github.com/pionxzh/wakaru/commit/40702d5397b0f2f456ac2329af49b35d0fff8a3e))
* auto rename component to be captialized ([51c3c65](https://github.com/pionxzh/wakaru/commit/51c3c65536ac91798716df218b077505c46f7276))
* **babel-helpers:** fix the replacement of array ([c540d39](https://github.com/pionxzh/wakaru/commit/c540d39ce8b5dfe85bb841434790fbf7f5ad94f4))
* **babel-helpers:** improve `arrayLikeToArray` matching ([bf9cfc7](https://github.com/pionxzh/wakaru/commit/bf9cfc7ef67deb1aae0766b33b41daa47ba38acb))
* **babel-helpers:** support finding in-file helpers ([4cef84a](https://github.com/pionxzh/wakaru/commit/4cef84a866fe8a005580ce132db1c7659ace456c))
* **babel-runtime:** slicedToArray no longer require single varaible declarator ([a01faab](https://github.com/pionxzh/wakaru/commit/a01faabdd8617316268b6ad0a3ba6b3d1a5a1c76))
* **babel-runtime:** support `createForOfIteratorHelper` ([f2c08a3](https://github.com/pionxzh/wakaru/commit/f2c08a3e1730cf27aabb919d33c69229eb150db6))
* better coverage for `un-if-statement` ([7cbdacf](https://github.com/pionxzh/wakaru/commit/7cbdacf9990da9b3d540b6d849a941bc3c5f06ec))
* better edge case handling for `un-if-statement` ([bd503c3](https://github.com/pionxzh/wakaru/commit/bd503c3e7c59d3b89c2bab67f387784e39c8e8e8))
* better edge handling for `transformToMultiStatementContext` ([954bcc6](https://github.com/pionxzh/wakaru/commit/954bcc6e573bf7f3a973dae4479550264e1fb299))
* better es6 class transformation ([0908c18](https://github.com/pionxzh/wakaru/commit/0908c18f5f8e7463488cf7913b3b8a5dd9b1e945))
* better handling of `delete` operator ([e10212a](https://github.com/pionxzh/wakaru/commit/e10212ace8d63f3d882444b4ea843abc59349c54))
* better scope handling for `un-export-rename` ([87877e3](https://github.com/pionxzh/wakaru/commit/87877e3d2b12e26ebe8bf61754476a559138ab5c))
* children in props should be move out ([b0cf169](https://github.com/pionxzh/wakaru/commit/b0cf1697ba856c7a05b19a466aa88cafd3d4a1b9))
* **cli:** make all call async and move default output folder to /out ([28dd8de](https://github.com/pionxzh/wakaru/commit/28dd8de7cd65fbfbdfcc87ad523e7c0df3ae04c8))
* **curly-braces:** avoid wrapping var declaration with BlockStatement ([8d452a6](https://github.com/pionxzh/wakaru/commit/8d452a6592edd77fd819bc4c59d5dc6e260857ae))
* **deps:** update babel monorepo ([f732bdd](https://github.com/pionxzh/wakaru/commit/f732bddccb9e1d48b5d054bd11a18caa3d1f855b))
* **deps:** update babel monorepo to ^7.22.11 ([90488c2](https://github.com/pionxzh/wakaru/commit/90488c2e36fb858767060d9e8408c012ebf56d97))
* **deps:** update babel monorepo to ^7.22.19 ([16ca888](https://github.com/pionxzh/wakaru/commit/16ca88885230210655f921c5a9193f298b600d58))
* **deps:** update babel monorepo to ^7.22.20 ([fc0311e](https://github.com/pionxzh/wakaru/commit/fc0311e5018f01d4796797c12b527dae7c34b346))
* **deps:** update babel monorepo to ^7.23.0 ([71682e0](https://github.com/pionxzh/wakaru/commit/71682e04d6be237b3eb0f334d6fd04498fed30d4))
* **deps:** update dependency @babel/preset-env to ^7.22.14 ([b524a7e](https://github.com/pionxzh/wakaru/commit/b524a7eff3618e79ab256f4e78b7c1c4ebc86826))
* **deps:** update dependency lebab to ^3.2.2 ([26a666f](https://github.com/pionxzh/wakaru/commit/26a666f482728dbcf5dffd1c2b0388f6d7a1b3e2))
* **deps:** update dependency lebab to ^3.2.3 ([0962ee0](https://github.com/pionxzh/wakaru/commit/0962ee03e7429265e2e0cf48b1aa59d16ba70eb5))
* disable `commonjs` and `multi-var` from `lebab` as they are problematic ([3a45b4e](https://github.com/pionxzh/wakaru/commit/3a45b4e3308dd1868c61317d764cfc5dfd7a86cb))
* fix reference check and identifier renaming ([244cb3e](https://github.com/pionxzh/wakaru/commit/244cb3ebfc89634b8b91c1efda7ecd4a687c6f94))
* fix type from `ast-types` ([50527d7](https://github.com/pionxzh/wakaru/commit/50527d708d1f17de1bae4d67fb650bbf84f86dd9))
* furthur improve scope handling and hoist behavior ([c321f93](https://github.com/pionxzh/wakaru/commit/c321f93e8b22eff484eced66c0eed2022ca1d610))
* handle empty variable declaration in `un-variable-merging` ([7fb8193](https://github.com/pionxzh/wakaru/commit/7fb8193311189bfcec9a243bdca4b68fa6a9d472))
* handle label-break-continue ([1f015e0](https://github.com/pionxzh/wakaru/commit/1f015e0c91d4f7528c23b2b312238aa55630c20d))
* improve coverage of optional chaining ([782a3a1](https://github.com/pionxzh/wakaru/commit/782a3a130a8d425dd3792b66b08c8e5e3dbb14ce))
* improve edge cases for `un-async-await` ([5568f2c](https://github.com/pionxzh/wakaru/commit/5568f2cfde76bf65b7dbbc079f57a0ad3bd54ac5))
* improve name generation and usage of scope ([6cf0140](https://github.com/pionxzh/wakaru/commit/6cf0140032a5bf282de5bb6f0a8695447da9c275))
* improve output condition by applying de morgan's law ([7b9153c](https://github.com/pionxzh/wakaru/commit/7b9153c1762a7a02a710b70eb61f8bf3133a0edf))
* **indirect-call:** should not limit require's source to be string ([c9f9eed](https://github.com/pionxzh/wakaru/commit/c9f9eeda1bbeb7fedf7e305d26a42eeb12e6f807))
* **indirect-call:** should reuse existing estructuring and make sure the position is correct ([279f843](https://github.com/pionxzh/wakaru/commit/279f84363748112fbaac79261c182f9dcf34de0e))
* **lebab:** do curly braces first to prevent lexical issue ([1d84d28](https://github.com/pionxzh/wakaru/commit/1d84d282a1eab418bacfbab596f19ba0a5602250))
* **lebab:** move lebab's parameter related rules to run after `un-parameters` ([3f375f7](https://github.com/pionxzh/wakaru/commit/3f375f762e0bef5f378432f9d4afcd107fd6b7fb))
* **module-mapping:** fix the filename mapping ([e058418](https://github.com/pionxzh/wakaru/commit/e05841864ed4612b84b07dd5e1fb8289d29c9cf0))
* moduleMapping should have a default value ([7da86cf](https://github.com/pionxzh/wakaru/commit/7da86cf617d3a22206ea6d8e38e8ca3f12ca06af))
* **parameter:** should check for "!" operator ([713b723](https://github.com/pionxzh/wakaru/commit/713b723c3808adc55dddbab64715c7120214902e))
* **parameter:** use proper ast matching instead of regex ([7160720](https://github.com/pionxzh/wakaru/commit/716072040ba250abdbe5429ec8167d87576a3fa0))
* reduce redundent parenthesis ([5e790a1](https://github.com/pionxzh/wakaru/commit/5e790a125f666bc9c64e799c255c431c01f6a7e7))
* refine the order of unminify rules ([128b019](https://github.com/pionxzh/wakaru/commit/128b0190772f90dcd170702302e44b20a6f8f443))
* rename `un-flip-operator` to `un-flip-comparisons` ([ffa2ed6](https://github.com/pionxzh/wakaru/commit/ffa2ed6a1e9417d3e61468a3d0fc974467d39828))
* rename `un-if-statement` to `un-conditionals` ([31e8174](https://github.com/pionxzh/wakaru/commit/31e8174b2b17ae25b7d86192d23559926c8645f0))
* run `un-variable-merging` earlier ([ad20e05](https://github.com/pionxzh/wakaru/commit/ad20e055e2f6327ac6de16fee8c1d4181548affe))
* **sequence-expression:** better comments retention ([c1f9abb](https://github.com/pionxzh/wakaru/commit/c1f9abb6dfe20e3071444f3c1768130b639bea66))
* should not convert to if-else in some case ([9336280](https://github.com/pionxzh/wakaru/commit/933628025a2338ae87e93ea7cf6c984a963a9c8e))
* **slicedToArray:** remove redundent clean function ([f0ea59c](https://github.com/pionxzh/wakaru/commit/f0ea59c2f3dbe5f9a720d3e46768b992180822cc))
* **smart-inline:** better comments retention ([05dc567](https://github.com/pionxzh/wakaru/commit/05dc567bcab560998619203eaf3ae092f5020e4e))
* **smart-inline:** fix stale scoping information ([ab5d695](https://github.com/pionxzh/wakaru/commit/ab5d695ba76e6b701863b1ad3b299298a1e5c6a8))
* **smart-inline:** respect correct declaration kind ([ca7e59c](https://github.com/pionxzh/wakaru/commit/ca7e59cd3566c535669b402fd15344a8588e5aa8))
* **smart-inline:** should run on body with 2 statements ([bd4ac04](https://github.com/pionxzh/wakaru/commit/bd4ac0413b7de3c60a5a2d763ebf00c7c00829ff))
* stay on prettier v2 for sync format ([a6b734e](https://github.com/pionxzh/wakaru/commit/a6b734ec55b6543861f1f79fc1f4dc533d0def62))
* transformation should break on error ([9b26f5a](https://github.com/pionxzh/wakaru/commit/9b26f5a5ca231e787cc602ae1dbe7cc1105cf513))
* **type-construct:** should not transform 0 length array ([376fd18](https://github.com/pionxzh/wakaru/commit/376fd18458531a9d8aaea44335194d052c83a969))
* un void should cover all literal ([13e2cf0](https://github.com/pionxzh/wakaru/commit/13e2cf0a9b577d92f743f9bfcfa5a23a765a094b))
* **un-esm:** should respect naming conflic when re-export identifier ([2990823](https://github.com/pionxzh/wakaru/commit/2990823ecc1524fc3d35448df55ec16d5fc4b2b5))
* **un-export-rename:** should respect naming conflic ([819a0df](https://github.com/pionxzh/wakaru/commit/819a0df3f714933f3b2d3340c941d8255db98728))
* **un-indirect-call:** improve import insertion ([7342846](https://github.com/pionxzh/wakaru/commit/7342846eacac0571481c907557879935ab7be069))
* **un-indirect-call:** improve importing and requiring variable ([45e8972](https://github.com/pionxzh/wakaru/commit/45e89720327a56d57c462921f41a028c4e760e3b))
* **un-indirect-call:** remove unused imports ([fa3c847](https://github.com/pionxzh/wakaru/commit/fa3c847be9112bf02d15063c584b70a9247e7c3b))
* **un-jsx:** fix capitalization detection ([13357cc](https://github.com/pionxzh/wakaru/commit/13357cceaa0b23b398df21976de8750ac62a2d1a))
* **un-jsx:** improve component rename ([5b6b094](https://github.com/pionxzh/wakaru/commit/5b6b09450f6ee44031ecfd6961b258d54c3f4f06))
* **un-jsx:** refactor and improve coverage ([00d8d09](https://github.com/pionxzh/wakaru/commit/00d8d09e6ebd717022bccd6cebc08a08d386384e))
* **un-undefined:** should not transform void 0 to undefined when undefined is declared ([149dd08](https://github.com/pionxzh/wakaru/commit/149dd08f2926dd320cb61b3d4f3ad15bf5bdbca0))
* **variable-merging:** should not limit the type of first declarator ([65f7a53](https://github.com/pionxzh/wakaru/commit/65f7a53db3251a8886791104d2fa4918681c1fe2))
* **while-loop:** better comments retention ([10ee0cd](https://github.com/pionxzh/wakaru/commit/10ee0cd8e8053ad1b8f6606c1e8f8df88ee56978))
* wip decision tree ([44929a5](https://github.com/pionxzh/wakaru/commit/44929a511c723968a5b52f9dd148991086d941bc))


### Performance Improvements

* reduce .filter ([ed24762](https://github.com/pionxzh/wakaru/commit/ed247623ef04a8502b39ce3110f8df540bb479cf))
* reduce redundent node checking ([5594b91](https://github.com/pionxzh/wakaru/commit/5594b91656927f33ba5841701556f959ee163a81))
