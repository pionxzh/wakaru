import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-flip-operator'

const inlineTest = defineInlineTest(transform)

inlineTest('should flip operators',
  `
undefined === foo;
null !== foo;
1 == foo;
true != foo;
"str" == foo;
\`test\` == foo;
NaN == foo;
Infinity == foo;
-Infinity == foo;
"function" == typeof foo;

1 < bar;
1 > bar;
1 <= bar;
1 >= bar;
`,
  `
foo === undefined;
foo !== null;
foo == 1;
foo != true;
foo == "str";
foo == \`test\`;
foo == NaN;
foo == Infinity;
foo == -Infinity;
typeof foo == "function";

bar > 1;
bar < 1;
bar >= 1;
bar <= 1;
`,
)

inlineTest('should not flip operators',
  `
foo === undefined;
foo !== null;
foo == 1;
foo != true;
foo == "str";
foo == \`test\`;
foo == \`test\${1}\`;
foo == NaN;
foo == Infinity;
typeof foo == "function";

({} == foo);
\`test\${1}\` == foo;

bar > 1;
bar < 1.2;
bar >= 1;
bar <= 1;
`,
  `
foo === undefined;
foo !== null;
foo == 1;
foo != true;
foo == "str";
foo == \`test\`;
foo == \`test\${1}\`;
foo == NaN;
foo == Infinity;
typeof foo == "function";

({} == foo);
\`test\${1}\` == foo;

bar > 1;
bar < 1.2;
bar >= 1;
bar <= 1;
`,
)
