import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../smart-rename'

const inlineTest = defineInlineTest(transform)

inlineTest('object destructuring rename',
  `
const {
  gql: t,
  dispatchers: o,
  listener: i
} = n;
o.delete(t, i);
`,
  `
const {
  gql,
  dispatchers,
  listener
} = n;
dispatchers.delete(gql, listener);
`,
)

inlineTest('react rename - createContext',
  `
const d = createContext(null);
const ef = o.createContext('light');

const g = o.createContext(a, b, c); // invalid parameters
const ThemeContext = o.createContext('light'); // name is not minified
`,
  `
const DContext = createContext(null);
const EfContext = o.createContext('light');

const g = o.createContext(a, b, c); // invalid parameters
const ThemeContext = o.createContext('light'); // name is not minified
`,
)

inlineTest('react rename - useState',
  `
const [e, f] = useState();
const [, g] = o.useState(0);

const h = o.useState(a, b); // invalid parameters
const size = o.useState(0); // name is not minified
`,
  `
const [e, setE] = useState();
const [, setG] = o.useState(0);

const h = o.useState(a, b); // invalid parameters
const size = o.useState(0); // name is not minified
`,
)

inlineTest('react rename - useRef',
  `
const d = useRef();
const ef = o.useRef(null);

const g = o.useRef(a, b); // invalid parameters
const ButtonRef = o.useRef(null); // name is not minified
`,
  `
const DRef = useRef();
const EfRef = o.useRef(null);

const g = o.useRef(a, b); // invalid parameters
const ButtonRef = o.useRef(null); // name is not minified
`,
)
