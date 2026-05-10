const { sharedValue } = require('./require-o-shared');

module.exports = function main() {
  return `entry:${sharedValue}`;
};
