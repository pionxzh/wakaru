const { Calculator, div, mul, pi } = require('./calculator')
const Greeting = require('./greeting')

const cal = new Calculator()
console.log(cal.add(pi, 2))
console.log(mul(1, 2))
console.log(div(1, 2))

const greeting = new Greeting('John')
greeting.sayHello()
