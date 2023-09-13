import { defineInlineTest, defineInlineTestWithOptions } from '@unminify-kit/test-utils'
import transform from '../un-jsx'

const inlineTest = defineInlineTest(transform)
const inlineTestWithOptions = defineInlineTestWithOptions(transform)

inlineTest('jsx',
  `
function fn() {
  return React.createElement("div", {
    className: "flex flex-col",
    num: 1,
    foo: bar,
    onClick: () => {},
  });
}
`,
  `
function fn() {
  return <div className="flex flex-col" num={1} foo={bar} onClick={() => {}} />;
}
`,
)

inlineTest('jsx no props',
  `
function fn() {
  return React.createElement("div", null);
}
`,
  `
function fn() {
  return <div />;
}
`,
)

inlineTest('jsx with spread child',
  `
function fn(children) {
  return React.createElement("div", {
    className: "flex flex-col",
  }, ...children);
}
`,
  `
function fn(children) {
  return (
    <div className="flex flex-col">
      {...children}
    </div>
  );
}
`,
)

inlineTest('jsx with child element',
  `
function fn() {
  return React.createElement(
    "div",
    { className: "flex flex-col" },
    React.createElement(
      "svg",
      { width: "24", height: "24" },
    ),
  );
}
`,
  `
function fn() {
  return (
    <div className="flex flex-col">
      <svg width="24" height="24" />
    </div>
  );
}
`,
)

inlineTest('jsx with children',
  `
function fn() {
  return React.createElement(
    "div",
    null,
    child,
    React.createElement('span', null, 'Hello'),
  );
}
`,
  `
function fn() {
  return (
    <div>
      {child}
      <span>Hello</span>
    </div>
  );
}
`,
)

inlineTest('jsx with empty children',
  `
function fn() {
  return React.createElement(
    "div",
    null,
    null,
    undefined,
    true,
    false,
  );
}
`,
  `
function fn() {
  return <div />;
}
`,
)

inlineTest('jsx with Component',
  `
function fn() {
  return React.createElement(Button, {
    variant: "contained",
  }, "Hello");
}
`,
  `
function fn() {
  return <Button variant="contained">Hello</Button>;
}
`,
)

inlineTest('jsx with Component #2',
  `
function fn() {
  return React.createElement(mui.Button, {
    variant: "contained",
  }, "Hello");
}
`,
  `
function fn() {
  return <mui.Button variant="contained">Hello</mui.Button>;
}
`,
)

inlineTest('jsx with child text that should be wrapped',
  `
function fn() {
  return React.createElement("div", null, ".style { color: red; }");
}
`,
  `
function fn() {
  return (
    <div>
      {".style { color: red; }"}
    </div>
  );
}
`,
)

inlineTest('jsx with a wrapped prop',
  `
function fn() {
  return React.createElement(
    "div",
    wrap({ className: "flex flex-col" }),
  );
}
`,
  `
function fn() {
  return <div {...wrap({ className: "flex flex-col" })} />;
}
`,
)

inlineTest('jsx assignment',
`
var div = /*#__PURE__*/React.createElement(Component, {
  ...props,
  foo: "bar"
});
`,
`
var div = <Component {...props} foo="bar" />;
`,
)

inlineTestWithOptions('jsx with custom pragma', { pragma: 'jsx' },
  `
function fn() {
  return jsx("div", null);
}
`,
  `
function fn() {
  return <div />;
}
`,
)

inlineTest('jsx with React.__spread',
  `
React.createElement("div", React.__spread({ key: "1" }, { className: "flex flex-col" }));
`,
  `
<div key="1" className="flex flex-col" />;
`,
)

inlineTest('jsx with Object.assign',
  `
React.createElement("div", Object.assign({ key: "1" }, { className: "flex flex-col" }));
`,
  `
<div key="1" className="flex flex-col" />;
`,
)

inlineTest('jsx with spread props',
  `
React.createElement("div", ...{ key: "1", className: "flex flex-col" });

`,
  `
<div key="1" className="flex flex-col" />;
`,
)

inlineTest('jsx props with escaped string',
  `
React.createElement(Foo, {bar: 'abc'});
React.createElement(Foo, {bar: 'a\\nbc'});
React.createElement(Foo, {bar: 'ab\\tc'});
React.createElement(Foo, {bar: 'ab"c'});
React.createElement(Foo, {bar: "ab'c"});
`,
  `
<Foo bar='abc' />;
<Foo bar={'a\\nbc'} />;
<Foo bar={'ab\\tc'} />;
<Foo bar={'ab"c'} />;
<Foo bar="ab'c" />;
`,
)

inlineTest('jsx props with mixed empty string',
  `
React.createElement("div", null, foo, ' ', bar);
`,
  `
<div>
  {foo}
  {' '}
  {bar}
</div>;
`,
)

inlineTest('jsx with bad capitalization tag',
  `
React.createElement(Foo, null);
React.createElement(foo, null);
React.createElement('Foo', null);
React.createElement('foo', null);
React.createElement(_foo, null);
React.createElement('_foo', null);
React.createElement(foo.bar, null);
`,
  `
<Foo />;
React.createElement(foo, null);
React.createElement('Foo', null);
<foo />;
<_foo />;
React.createElement('_foo', null);
<foo.bar />;
`,
)

inlineTest('jsx with xml namespace',
  `
h("f:image", {
  "n:attr": true
});
`,
  `
<f:image n:attr />;
`,
)

inlineTest('Babel: concatenates-adjacent-string-literals',
`
var x = /*#__PURE__*/React.createElement("div", null, "foo", "bar", "baz", /*#__PURE__*/React.createElement("div", null, "buz bang"), "qux", null, "quack");
`,
`
var x = <div>
  foo
  bar
  baz
  <div>buz bang</div>
  qux
  quack
</div>;
`,
)

inlineTest('Babel: does-not-add-source-self',
`
var x = /*#__PURE__*/React.createElement(React.Fragment, null, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("div", {
  key: "1"
}), /*#__PURE__*/React.createElement("div", {
  key: "2",
  meow: "wolf"
}), /*#__PURE__*/React.createElement("div", {
  key: "3"
}), /*#__PURE__*/React.createElement("div", {
  ...props,
  key: "4"
})));
`,
`
var x = <>
  <div>
    <div key="1" />
    <div key="2" meow="wolf" />
    <div key="3" />
    <div {...props} key="4" />
  </div>
</>;
`,
)

inlineTest('Babel: dont-coerce-expression-containers',
`
React.createElement(Text, null, "To get started, edit index.ios.js!!!", "\\n", "Press Cmd+R to reload");
`,
`
<Text>
  To get started, edit index.ios.js!!!
  {"\\n"}
  Press Cmd+R to reload
</Text>;
`,
)

inlineTest('Babel: duplicate-props',
`
/*#__PURE__*/React.createElement("p", {
  prop: true,
  prop: true
}, "text");
/*#__PURE__*/React.createElement("p", {
  prop,
  prop
  }, "text");
/*#__PURE__*/React.createElement("p", {
  prop: true,
  prop
  }, "text");
/*#__PURE__*/React.createElement("p", {
  prop,
  prop: true
}, "text");
`,
`
<p prop prop>text</p>;
<p prop={prop} prop={prop}>text</p>;
<p prop prop={prop}>text</p>;
<p prop={prop} prop>text</p>;
`,
)

inlineTest('Babel: flattens-spread',
`
/*#__PURE__*/React.createElement("p", props, "text");
/*#__PURE__*/React.createElement("div", props, contents);
/*#__PURE__*/React.createElement("img", {
  alt: "",
  src,
  title,
  __proto__
});
/*#__PURE__*/React.createElement("blockquote", {
  cite
}, items);
/*#__PURE__*/React.createElement("pre", {
  ["__proto__"]: null
});
/*#__PURE__*/React.createElement("code", {
  [__proto__]: null
});
`,
`
<p {...props}>text</p>;
<div {...props}>
  {contents}
</div>;
<img alt="" src={src} title={title} __proto__={__proto__} />;
<blockquote cite={cite}>
  {items}
</blockquote>;
<pre
  {...{
    ["__proto__"]: null
  }} />;
<code
  {...{
    [__proto__]: null
  }} />;
`,
)

inlineTest('Babel: handle-spread-with-proto',
`
/*#__PURE__*/React.createElement("p", {
  ...{
    __proto__: null
  }
}, "text");
/*#__PURE__*/React.createElement("div", {
  ...{
    "__proto__": null
  }
}, contents);
`,
`
<p
  {...{
    __proto__: null
  }}>text</p>;
<div
  {...{
    "__proto__": null
  }}>
  {contents}
</div>;
`,
)

inlineTest('Babel: should-add-quote-es3',
  `
var es3 = /*#__PURE__*/React.createElement(F, {
  aaa: true,
  "new": true,
  "const": true,
  "var": true,
  "default": true,
  "foo-bar": true
});
`,
  `
var es3 = <F aaa new const var default foo-bar />;
`,
)

inlineTest('Babel: should-allow-deeper-js-namespacing',
  `
/*#__PURE__*/React.createElement(Namespace.DeepNamespace.Component, null);
`,
  `
<Namespace.DeepNamespace.Component />;
`,
)

inlineTest('Babel: should-allow-elements-as-attributes',
  `
/*#__PURE__*/React.createElement("div", {
  attr: /*#__PURE__*/React.createElement("div", null)
});
`,
  `
<div attr={<div />} />;
`,
)

inlineTestWithOptions('Babel: should-allow-pragmaFrag-and-frag', { pragma: 'dom', pragmaFrag: 'DomFrag' },
  `
dom(DomFrag, null);
`,
  `
<></>;
`,
)
