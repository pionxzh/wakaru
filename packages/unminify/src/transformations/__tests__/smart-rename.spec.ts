import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../smart-rename'

const inlineTest = defineInlineTest(transform)

inlineTest('object destructuring rename',
  `
const {
  gql: t,
  dispatchers: o,
  listener: i = noop,
  sameName: sameName
} = n;
o.delete(t, i);
`,
  `
const {
  gql,
  dispatchers,
  listener = noop,
  sameName
} = n;
dispatchers.delete(gql, listener);
`,
)

inlineTest('object destructuring in function parameter',
  `
function foo({
  gql: t,
  dispatchers: o,
  listener: i
}) {
  o.delete(t, i);
}

const foo2 = ({
  gql: t,
  dispatchers: o,
  listener: i
}) => {
  t[o].delete(i);
}

const foo3 = function ({
  gql: t,
  dispatchers: o,
  listener: i
}) {
  o.delete(t, i);
}

const foo4 = {
  foo({
    gql: t,
    dispatchers: o,
    listener: i
  }) {
    o.delete(t, i);
  }
}

class Foo {
  constructor({
    gql: t,
    dispatchers: o,
    listener: i
  }) {
    o.delete(t, i);
  }

  foo({
    gql: t,
    dispatchers: o,
    listener: i
  }) {
    o.delete(t, i);
  }
}
`,
  `
function foo({
  gql,
  dispatchers,
  listener
}) {
  dispatchers.delete(gql, listener);
}

const foo2 = ({
  gql,
  dispatchers,
  listener
}) => {
  gql[dispatchers].delete(listener);
}

const foo3 = function ({
  gql,
  dispatchers,
  listener
}) {
  dispatchers.delete(gql, listener);
}

const foo4 = {
  foo({
    gql,
    dispatchers,
    listener
  }) {
    dispatchers.delete(gql, listener);
  }
}

class Foo {
  constructor({
    gql,
    dispatchers,
    listener
  }) {
    dispatchers.delete(gql, listener);
  }

  foo({
    gql,
    dispatchers,
    listener
  }) {
    dispatchers.delete(gql, listener);
  }
}
`,
)

inlineTest('object destructuring in function parameter with naming conflict',
  `
const gql = 1;

function foo({
  gql: t,
  dispatchers: o,
  listener: i
}, {
  gql: a,
  dispatchers: b,
  listener: c
}) {
  o.delete(t, i, a, b, c);
}
`,
  `
const gql = 1;

function foo({
  gql: gql_1,
  dispatchers,
  listener
}, {
  gql: gql_2,
  dispatchers: dispatchers_1,
  listener: listener_1
}) {
  dispatchers.delete(gql_1, listener, gql_2, dispatchers_1, listener_1);
}
`,
)

inlineTest('object destructuring with reserved identifier',
  `
const {
  static: t,
  default: o,
} = n;
o.delete(t);
`,
  `
const {
  static: _static,
  default: _default,
} = n;
_default.delete(_static);
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
`,
  `
const [e, setE] = useState();
const [, setG] = o.useState(0);

const h = o.useState(a, b); // invalid parameters
`,
)

inlineTest('react rename - useReducer',
  `
const [e, f] = useReducer(r, i);
const [g, h] = o.useReducer(r, i, init);

const k = o.useReducer(a); // invalid parameters
`,
  `
const [eState, fDispatch] = useReducer(r, i);
const [gState, hDispatch] = o.useReducer(r, i, init);

const k = o.useReducer(a); // invalid parameters
`,
)

inlineTest('react rename - useRef',
  `
const d = useRef();
const ef = o.useRef(null);

const g = o.useRef(a, b); // invalid parameters
const buttonRef = o.useRef(null); // name is not minified
`,
  `
const dRef = useRef();
const efRef = o.useRef(null);

const g = o.useRef(a, b); // invalid parameters
const buttonRef = o.useRef(null); // name is not minified
`,
)

inlineTest('react rename - forwardRef',
  `
const Z = forwardRef((e, t) => {
  return (
    <label>
      {e.label}
      <input ref={t} />
    </label>
  );
});
const X = o.forwardRef(function (e, ref2) {
  return (
    <label>
      {e.label}
      <input ref={ref2} />
    </label>
  );
});

const Y = o.forwardRef(a, b); // invalid parameters
`,
  `
const Z = forwardRef((props, ref) => {
  return (
    (<label>
      {props.label}
      <input ref={ref} />
    </label>)
  );
});
const X = o.forwardRef(function (props, ref2) {
  return (
    (<label>
      {props.label}
      <input ref={ref2} />
    </label>)
  );
});

const Y = o.forwardRef(a, b); // invalid parameters
`,
)
