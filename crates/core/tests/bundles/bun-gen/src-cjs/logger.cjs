class Logger {
  constructor(prefix) {
    this._prefix = prefix || '';
  }
  info(msg) {
    console.log('[INFO] ' + this._prefix + msg);
  }
  warn(msg) {
    console.warn('[WARN] ' + this._prefix + msg);
  }
}

module.exports = Logger;
module.exports.Logger = Logger;
