const isArray = require('is-array')

const pi = 3.14

module.exports.pi = pi

class Calculator {
    add(a, b) {
        return a + b
    }

    sub(a, b) {
        return a - b
    }

    sum(arr) {
        if (!isArray(arr)) throw new Error('Argument must be an array')
        return arr.reduce((a, b) => a + b, 0)
    }

    pi() {
        return pi
    }
}

module.exports.Calculator = Calculator

module.exports.mul = (a, b) => a * b

function div(a, b) {
    return a / b
}

module.exports.div = div

const isArr = arr => isArray(arr)

module.exports.isArr = isArr
