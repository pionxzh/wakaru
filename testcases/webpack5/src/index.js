import M1 from './1.js'
import { A } from './a.js'
import b, { version } from './b.js'
import { getC } from './c.js'
import { A as AA } from './d.js'
import { A as AAA } from './e.js'

const d = new AA()
const e = new AAA()

console.log(version, A, d, e)

b()

getC().then(console.log)

// const M1 = await import('./1.js')

M1()
