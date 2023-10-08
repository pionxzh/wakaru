import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-indirect-call'

const inlineTest = defineInlineTest(transform)

inlineTest('indirect call from a imported module',
  `
import s from "react";

var countRef = (0, s.useRef)(0);
`,
  `
import { useRef } from "react";

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
import { useRef } from "react";
import { useRef as useRef$0 } from "another";

var countRef = useRef(0);
var secondRef = useRef$0(0);
var thirdRef = useRef$0(0);
`,
)

inlineTest('indirect call from a required module',
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

inlineTest('indirect call from required module with existing destructuring',
  `
const s = require("react");
const { useRef } = s;

var countRef = (0, s.useRef)(0);
var secondRef = (0, s.useMemo)(() => {}, []);
`,
  `
const s = require("react");
const {
  useRef,
  useMemo
} = s;

var countRef = useRef(0);
var secondRef = useMemo(() => {}, []);
`,
)

inlineTest('indirect call from required module with existing destructuring declared after',
  `
const s = require("react");

var countRef = (0, s.useRef)(0);
var secondRef = (0, s.useMemo)(() => {}, []);

const { useRef } = s;
`,
  `
const s = require("react");

const {
  useRef: useRef$0,
  useMemo
} = s;

var countRef = useRef$0(0);
var secondRef = useMemo(() => {}, []);

const { useRef } = s;
`,
)

inlineTest('indirect call from multiple required modules',
  `
const s = require("react");
const t = require(9527);

var countRef = (0, s.useRef)(0);
var secondRef = (0, t.useRef)(0);
var thirdRef = (0, t.useRef)(0);
`,
  `
const s = require("react");

const {
  useRef
} = s;

const t = require(9527);

const {
  useRef: useRef$0
} = t;

var countRef = useRef(0);
var secondRef = useRef$0(0);
var thirdRef = useRef$0(0);
`,
)
