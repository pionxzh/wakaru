import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-template-literal'

const inlineTest = defineInlineTest(transform)

inlineTest('should restore template literal syntax',
  `
var example1 = "the ".concat("simple ", form);
var example2 = "".concat(1);
var example3 = 1 + "".concat(foo).concat(bar).concat(baz);
var example4 = 1 + "".concat(foo, "bar").concat(baz);
var example5 = "".concat(1, f, "oo", true).concat(b, "ar", 0).concat(baz);
var example6 = "test ".concat(foo, " ").concat(bar);
`,
  `
var example1 = \`the simple \${form}\`;
var example2 = \`\${1}\`;
var example3 = 1 + \`\${foo}\${bar}\${baz}\`;
var example4 = 1 + \`\${foo}bar\${baz}\`;
var example5 = \`\${1}\${f}oo\${true}\${b}ar\${0}\${baz}\`;
var example6 = \`test \${foo} \${bar}\`;
`,
)

inlineTest('multiple arguments',
  `
"the ".concat(first, " take the ").concat(second, " and ").concat(third);
`,
  `
\`the \${first} take the \${second} and \${third}\`;
`,
)

inlineTest('escaped quotes',
  `
"'".concat(foo, "' \\"").concat(bar, "\\"");
`,
  `
\`'\${foo}' "\${bar}"\`;
`,
)

inlineTest('escape backticks and dollar signs',
  `
const codeBlock = "\`\`\`".concat(lang, "\\n").concat(code, "\\n\`\`\`");
`,
  `
const codeBlock = \`\\\`\\\`\\\`\${lang}\\n\${code}\\n\\\`\\\`\\\`\`;
`,
)

inlineTest('should keep non-consecutive-concat calls',
  `
"the".concat(first, " take the ").concat(second, " and ").split(' ').concat(third);
`,
  `
\`the\${first} take the \${second} and \`.split(' ').concat(third);
`,
)
