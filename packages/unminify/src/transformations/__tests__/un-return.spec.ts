import transform from '../un-return'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('transform return void expr to expr',
  `
function foo() {
  return void a()
}
`,
  `
function foo() {
  a();
}
`,
)

inlineTest('remove redundant return',
  `
function foo() {
  const a = 1
  return undefined
}

const bar = () => {
  const a = 1
  if (a) return void 0
  return void 0
}

const baz = function () {
  const a = 1
  if (a) {
    return undefined
  }
  return undefined
}

const obj = {
  method() {
    const a = 1
    return void 0
  }
}

class A {
  method() {
    const a = 1
    return
  }
}
`,
  `
function foo() {
  const a = 1
}

const bar = () => {
  const a = 1
  if (a) return void 0
}

const baz = function () {
  const a = 1
  if (a) {
    return undefined
  }
}

const obj = {
  method() {
    const a = 1
  }
}

class A {
  method() {
    const a = 1
  }
}
`,
)

/**
 * Normally bundler and normal human will not write this kind of code.
 * We put it here just to show that we won't do anything special here.
 */
inlineTest('double return',
  `
function foo() {
  return void 0
  return undefined
}
`,
  `
function foo() {
  return void 0
}
`,
)

inlineTest('should not transform the following cases',
  `
function foo() {
  const count = 5;
  while (count--) {
    return void 0;
  }

  for (let i = 0; i < 10; i++) {
    return void foo();
  }
}
`,
  `
function foo() {
  const count = 5;
  while (count--) {
    return void 0;
  }

  for (let i = 0; i < 10; i++) {
    return void foo();
  }
}
`,
)
