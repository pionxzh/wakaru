import { add, PI } from './math.js';
import { greet } from './greet.js';
import { clamp } from './utils-cjs.cjs';
import { padLeft } from './format-cjs.cjs';
import { isEmail } from './validate-cjs.cjs';
import { toUpper } from './convert-cjs.cjs';
import { unique } from './array-cjs.cjs';
console.log(greet("world"), add(1, PI), clamp(5, 0, 10), padLeft("x", 5), isEmail("a@b"), toUpper("hi"), unique([1,2,2]));
