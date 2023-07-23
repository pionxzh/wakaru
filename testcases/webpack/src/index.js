import { A } from './a.js'
import b, { version } from './b.js'
import { getC } from './c.js'
import M1 from './1.js'

console.log(version, A)

b()

getC().then(console.log)

M1()
