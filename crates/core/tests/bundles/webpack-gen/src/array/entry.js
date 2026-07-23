const m01 = require('./m01.js');
const m02 = require('./m02.js');
const m03 = require('./m03.js');
const m04 = require('./m04.js');
const m05 = require('./m05.js');
const m06 = require('./m06.js');
const m07 = require('./m07.js');
const m08 = require('./m08.js');
const m09 = require('./m09.js');
const m10 = require('./m10.js');
const m11 = require('./m11.js');
const m12 = require('./m12.js');
const m13 = require('./m13.js');
const m14 = require('./m14.js');
const m15 = require('./m15.js');
const m16 = require('./m16.js');
const m17 = require('./m17.js');

const total = m01.value + m02.value + m03.value + m04.value + m05.value + m06.value + m07.value + m08.value + m09.value + m10.value + m11.value + m12.value + m13.value + m14.value + m15.value + m16.value + m17.value;
console.log('static total', total);

import('./lazy/l1.js').then((lazy) => {
    console.log('lazy total', lazy.total);
});
