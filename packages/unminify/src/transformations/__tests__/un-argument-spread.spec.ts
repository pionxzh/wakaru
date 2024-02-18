import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-argument-spread'

const inlineTest = defineInlineTest(transform)

inlineTest('should convert plain fn.apply()',
  `
fn.apply(undefined, someArray);
fn.apply(null, someArray);
`,
  `
fn(...someArray);
fn(...someArray);
`,
)

inlineTest('should not convert plain fn.apply() when actual object used as this parameter',
  `
fn.apply(obj, someArray);
fn.apply(this, someArray);
fn.apply({}, someArray);
`,
  `
fn.apply(obj, someArray);
fn.apply(this, someArray);
fn.apply({}, someArray);
`,
)

inlineTest('should convert basic obj.fn.apply()',
  `
obj.fn.apply(obj, someArray);

class T {
  fn() {
    this.fn.apply(this, someArray);
  }
}
`,
  `
obj.fn(...someArray);

class T {
  fn() {
    this.fn(...someArray);
  }
}
`,
)

inlineTest('should convert obj[fn].apply()',
  `
obj[fn].apply(obj, someArray);
`,
  `
obj[fn](...someArray);
`,
)

inlineTest('should not convert obj.fn.apply() without obj as parameter',
  `
obj.fn.apply(otherObj, someArray);
obj.fn.apply(undefined, someArray);
obj.fn.apply(void 0, someArray);
obj.fn.apply(null, someArray);
obj.fn.apply(this, someArray);
obj.fn.apply({}, someArray);
`,
  `
obj.fn.apply(otherObj, someArray);
obj.fn.apply(undefined, someArray);
obj.fn.apply(void 0, someArray);
obj.fn.apply(null, someArray);
obj.fn.apply(this, someArray);
obj.fn.apply({}, someArray);
`,
)

inlineTest('should convert obj.fn.apply() with array expression',
  `
obj.fn.apply(obj, [1, 2, 3]);
`,
  `
obj.fn(...[1, 2, 3]);
`,
)

inlineTest('should convert <long-expression>.fn.apply()',
  `
foo[bar+1].baz.fn.apply(foo[bar+1].baz, someArray);
`,
  `
foo[bar+1].baz.fn(...someArray);
`,
)

inlineTest('should convert <literal>.fn.apply()',
  `
[].fn.apply([], someArray);
`,
  `
[].fn(...someArray);
`,
)
