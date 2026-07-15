const bar = require("./ncc-bar.cjs");

function greet(name) {
  const formattedName = bar.formatName(name);
  return `Hello ${formattedName}!`;
}

module.exports = { greet };
