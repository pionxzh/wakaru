const { add, pi } = require('./calculator')

class Greeting {
    constructor(name) {
        this.name = name
    }

    sayHello() {
        console.log(`Hello ${this.name}. Your lucky number is ${add(pi, 2)}`)
    }
}

module.exports = Greeting
