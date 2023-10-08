import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-template-literal'

const inlineTest = defineInlineTest(transform)

inlineTest('should restore template literal syntax',
`
"the ".concat("simple ", form);

"the ".concat(first, " take the ").concat(second, " and ").concat(third);
`,
`
\`the simple \${form}\`;

\`the \${first} take the \${second} and \${third}\`;
`,
)

inlineTest('should not touch non-consecutive-concat calls',
`
"the".concat(first, " take the ").concat(second, " and ").split(' ').concat(third);
`,
`
\`the\${first} take the \${second} and \`.split(' ').concat(third);
`,
)
