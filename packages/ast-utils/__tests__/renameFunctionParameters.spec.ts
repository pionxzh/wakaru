import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import { defineInlineTest } from '@wakaru/test-utils'
import { renameFunctionParameters } from '../src/renameFunctionParameters'

const transform = createJSCodeshiftTransformationRule({
    name: 'test-rename-function-parameters',
    transform: (context) => {
        const { root, j } = context

        root
            .find(j.FunctionDeclaration)
            .forEach((path) => {
                const node = path.node
                renameFunctionParameters(j, node, ['c', 'd', 'xx', 'zz'])
            })
    },
})

const inlineTest = defineInlineTest(transform)

inlineTest('should rename function parameters',
  `
function foo(a, b) {
  const obj = {
    a: a.a,
    b: b.c,
    c: e.b,
  }

  let f = (Y) => {
    let {
      ...a
    } = Y
  }

  return a + b;
}
`,
  `
function foo(c, d) {
  const obj = {
    a: c.a,
    b: d.c,
    c: e.b,
  }

  let f = (Y) => {
    let {
      ...a
    } = Y
  }

  return c + d;
}
`,
)

inlineTest('should rename function parameters #2',
  `
function foo(a, b) {
  function a() {
    return a + b;
  }

  return a + b;
}
`,
  `
function foo(c, d) {
  function a() {
    return a + d;
  }

  return a + d;
}
`,
)

inlineTest('should rename function parameters #3',
  `
function z(e, t, n) {
  "use strict";
  !(function e() {
    if (
      "undefined" != typeof __REACT_DEVTOOLS_GLOBAL_HOOK__ &&
      "function" == typeof __REACT_DEVTOOLS_GLOBAL_HOOK__.checkDCE
    )
      try {
        __REACT_DEVTOOLS_GLOBAL_HOOK__.checkDCE(e);
      } catch (e) {
        console.error(e);
      }
  })();
  e.exports = n(48);
}
`,
  `
function z(c, d, xx) {
  "use strict";
  !(function e() {
    if (
      "undefined" != typeof __REACT_DEVTOOLS_GLOBAL_HOOK__ &&
      "function" == typeof __REACT_DEVTOOLS_GLOBAL_HOOK__.checkDCE
    )
      try {
        __REACT_DEVTOOLS_GLOBAL_HOOK__.checkDCE(e);
      } catch (e) {
        console.error(e);
      }
  })();
  c.exports = xx(48);
}
`,
)

inlineTest('Class',
  `
function foo(a, b, x, z) {
  class Bar {
    constructor(a, b) {
      this.a = a;
      this.b = b;
    }

    #x = x;
    #z = this.#x;

    #a(a, b) {
      return this.#x + b;
    }

    #b(f, g) {
      return this.#a(this.z, this.b);
    }
  }
}
`,
  `
function foo(c, d, xx, zz) {
  class Bar {
    constructor(a, b) {
      this.a = a;
      this.b = b;
    }

    #x = xx;
    #z = this.#x;

    #a(a, b) {
      return this.#x + b;
    }

    #b(f, g) {
      return this.#a(this.z, this.b);
    }
  }
}
`,
)
