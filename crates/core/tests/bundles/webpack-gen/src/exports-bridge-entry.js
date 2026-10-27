const Lib = require("./exports-bridge-iife.cjs");
const second = require("./exports-bridge-reversed.cjs");
const styles = require("./exports-bridge-if.cjs");

console.log(new Lib("alpha").first(), Lib.Lib === Lib);
console.log(second.first, second.second);
console.log(styles.alpha, styles.beta);
