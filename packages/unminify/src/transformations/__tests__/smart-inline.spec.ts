import { defineInlineTest } from '@unminify-kit/test-utils'
import transform from '../smart-inline'

const inlineTest = defineInlineTest(transform)

inlineTest('property destructuring #1',
  `
const t = e.x;
const n = e.y;
const r = e.color;
e.type;
console.log(t, n, r);
`,
  `
const {
  x,
  y,
  color,
  type
} = e;

console.log(x, y, color);
`,
)

inlineTest('property destructuring #2',
  `
const t = e.size;
const n = e.size;
const r = e.color;
const g = e.color;

console.log(t, n, r, g);
`,
  `
const {
  size,
  color
} = e;

console.log(size, size, color, color);
`,
)

inlineTest('property destructuring #3',
  `
const n = e.size;
const r = e.color;

const t = f.size;
const g = f.color;

console.log(n, r, t, g);
`,
  `
const {
  size,
  color
} = e;

const {
  size: size$0,
  color: color$0
} = f;

console.log(size, color, size$0, color$0);
`,
)

inlineTest('array destructuring #1',
  `
const t = e[0];
const n = e[1];
const r = e[2];
console.log(t, n, r);
`,
  `
const [t, n, r] = e;
console.log(t, n, r);
`,
)

inlineTest('array destructuring #2',
  `
const t = e[0];
const n = e[2];
const r = e[4];
const g = e[99];
console.log(t, n, r, g);
`,
  `
const [t,, n,, r] = e;
const g = e[99];
console.log(t, n, r, g);
`,
)
