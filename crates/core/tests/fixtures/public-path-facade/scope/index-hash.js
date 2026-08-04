function helperA1() { return 1; }
function helperA2() { return helperA1() + 1; }
function helperA3() { return helperA2() * 2; }
function helperA4() { return helperA3() + 3; }
function publicA() { return helperA4(); }

function helperB1() { return 10; }
function helperB2() { return helperB1() + 10; }
function helperB3() { return helperB2() * 20; }
function helperB4() { return helperB3() + 30; }
function publicB() { return helperB4(); }

const result = publicA() + publicB();
console.log(result);

export let liveValue = 1;
export default function readDefault() { return publicA(); }
export { publicA as namedValue, publicB };
export * from "./left.js";
export * from "./right.js";
liveValue += 1;
