import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-flip-comparisons'

const inlineTest = defineInlineTest(transform)

inlineTest('should flip operators',
  `
void 0 === foo;
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
foo === void 0;
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

inlineTest('should flip operators for various right-hand values',
  `
1 == obj.props;
1 == obj.props[0];
1 == method();
`,
  `
obj.props == 1;
obj.props[0] == 1;
method() == 1;
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
