import { defineInlineTestWithOptions } from '@unminify-kit/test-utils'
import transform from '../un-indirect-call'

const inlineTest = defineInlineTestWithOptions(transform)

inlineTest('indirect call from a imported module', {},
  `
import s from 'react';

var count = (0, s.useRef)(0);
`,
  `
import s, { useRef } from 'react';

var count = useRef(0);
`,
)

inlineTest('indirect call from a required module', { unsafe: true },
  `
const s = require('react');

var count = (0, s.useRef)(0);
`,
  `
const s = require('react');

const {
  useRef
} = s;

var count = useRef(0);
`,
)

inlineTest('indirect call from a required module', { unsafe: true },
  `
const s = require('react');
const { useRef } = s;

var count = (0, s.useRef)(0);
`,
  `
const s = require('react');
const { useRef } = s;

var count = useRef(0);
`,
)
