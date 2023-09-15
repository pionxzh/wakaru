import { defineInlineTest, defineInlineTestWithOptions } from '@unminify-kit/test-utils'
import transform from '../un-indirect-call'

const inlineTest = defineInlineTest(transform)
const inlineTestWithOptions = defineInlineTestWithOptions(transform)

inlineTest('indirect call from a imported module',
  `
import s from "react";

var countRef = (0, s.useRef)(0);
`,
  `
import s, { useRef } from "react";

var countRef = useRef(0);
`,
)

inlineTest('multiple indirect call from different sources',
  `
import s from "react";
import t from "another";

var countRef = (0, s.useRef)(0);
var secondRef = (0, t.useRef)(0);
var thirdRef = (0, t.useRef)(0);
`,
  `
import s, { useRef } from "react";
import t, { useRef as useRef$0 } from "another";

var countRef = useRef(0);
var secondRef = useRef$0(0);
var thirdRef = useRef$0(0);
`,
)

inlineTestWithOptions('indirect call from a required module', { unsafe: true },
  `
const s = require("react");

var countRef = (0, s.useRef)(0);
`,
  `
const s = require("react");

const {
  useRef
} = s;

var countRef = useRef(0);
`,
)

inlineTestWithOptions('indirect call from a required module with existing destructuring', { unsafe: true },
  `
const s = require("react");
const { useRef } = s;

var countRef = (0, s.useRef)(0);
`,
  `
const s = require("react");
const { useRef } = s;

var countRef = useRef(0);
`,
)

inlineTestWithOptions('indirect call from a required module with existing destructuring', { unsafe: true },
  `
const s = require("react");
const t = require("another");

var countRef = (0, s.useRef)(0);
var secondRef = (0, t.useRef)(0);
var thirdRef = (0, t.useRef)(0);
`,
  `
const s = require("react");

const {
  useRef
} = s;

const t = require("another");

const {
  useRef: useRef$0
} = t;

var countRef = useRef(0);
var secondRef = useRef$0(0);
var thirdRef = useRef$0(0);
`,
)
