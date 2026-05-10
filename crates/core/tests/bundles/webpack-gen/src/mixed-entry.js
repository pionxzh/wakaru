import { version } from './version';
const greetMod = require('./greet');

console.log(version, greetMod.greet('world'));
