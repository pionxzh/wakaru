(function(){function r(e,n,t){function o(i,f){if(!n[i]){if(!e[i]){var c="function"==typeof require&&require;if(!f&&c)return c(i,!0);if(u)return u(i,!0);var a=new Error("Cannot find module '"+i+"'");throw a.code="MODULE_NOT_FOUND",a}var p=n[i]={exports:{}};e[i][0].call(p.exports,function(r){var n=e[i][1][r];return o(n||r)},p,p.exports,r,e,n,t)}return n[i].exports}for(var u="function"==typeof require&&require,i=0;i<t.length;i++)o(t[i]);return o}return r})()({1:[function(require,module,exports){

/**
 * isArray
 */

var isArray = Array.isArray;

/**
 * toString
 */

var str = Object.prototype.toString;

/**
 * Whether or not the given `val`
 * is an array.
 *
 * example:
 *
 *        isArray([]);
 *        // > true
 *        isArray(arguments);
 *        // > false
 *        isArray('');
 *        // > false
 *
 * @param {mixed} val
 * @return {bool}
 */

module.exports = isArray || function (val) {
  return !! val && '[object Array]' == str.call(val);
};

},{}],2:[function(require,module,exports){
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

},{"is-array":1}],3:[function(require,module,exports){
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

},{"./calculator":2}],4:[function(require,module,exports){
const { Calculator, div, mul, pi } = require('./calculator')
const Greeting = require('./greeting')

const cal = new Calculator()
console.log(cal.add(pi, 2))
console.log(mul(1, 2))
console.log(div(1, 2))

const greeting = new Greeting('John')
greeting.sayHello()

},{"./calculator":2,"./greeting":3}]},{},[4]);
