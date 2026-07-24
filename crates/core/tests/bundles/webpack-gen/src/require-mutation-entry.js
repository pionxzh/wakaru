__webpack_public_path__ = '/entry-owned/';
__webpack_require__.instrumentedBefore = true;

const hooks = {
  get value() {
    __webpack_require__.instrumentedFromGetter = true;
    return 'hook';
  },
};

console.log(require('./require-mutation-dep.cjs')('review'), hooks.value);
__webpack_require__.instrumentedAfter = true;
