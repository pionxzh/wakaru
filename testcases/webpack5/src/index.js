import M1 from './1.js'
import { A } from './a.js'
import b, { version } from './b.js'
import { getC } from './c.js'

console.log(version, A)

b()

getC().then(console.log)

// const M1 = await import('./1.js')

M1()
