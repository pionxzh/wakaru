import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-typeof'

const inlineTest = defineInlineTest(transform)

inlineTest('transform typeof',
  `
typeof x < "u";
"u" > typeof x;
typeof x > "u";
"u" < typeof x;
`,
  `
typeof x !== "undefined";
typeof x !== "undefined";
typeof x === "undefined";
typeof x === "undefined";
`,
)

inlineTest('should not transform typeof',
  `
typeof x <= "u";
typeof x >= "u";
typeof x === "string";
typeof x === "number";
typeof x === "boolean";
typeof x === "symbol";
typeof x === "object";
typeof x === "bigint";
typeof x === "function";
typeof x === "undefined";
`,
  `
typeof x <= "u";
typeof x >= "u";
typeof x === "string";
typeof x === "number";
typeof x === "boolean";
typeof x === "symbol";
typeof x === "object";
typeof x === "bigint";
typeof x === "function";
typeof x === "undefined";
`,
)
