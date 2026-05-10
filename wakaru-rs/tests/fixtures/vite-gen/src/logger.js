export const LogLevel = Object.freeze({
  DEBUG: 0,
  INFO: 1,
  WARN: 2,
  ERROR: 3,
});

export class Logger {
  constructor(level = LogLevel.INFO, prefix = '') {
    this._level = level;
    this._prefix = prefix;
  }

  _format(level, message) {
    const ts = new Date().toISOString();
    const name = Object.keys(LogLevel).find((k) => LogLevel[k] === level) || 'UNKNOWN';
    const parts = [ts, `[${name}]`];
    if (this._prefix) parts.push(`(${this._prefix})`);
    parts.push(message);
    return parts.join(' ');
  }

  debug(msg, ...args) {
    if (this._level <= LogLevel.DEBUG) console.debug(this._format(LogLevel.DEBUG, msg), ...args);
  }

  info(msg, ...args) {
    if (this._level <= LogLevel.INFO) console.info(this._format(LogLevel.INFO, msg), ...args);
  }

  warn(msg, ...args) {
    if (this._level <= LogLevel.WARN) console.warn(this._format(LogLevel.WARN, msg), ...args);
  }

  error(msg, ...args) {
    if (this._level <= LogLevel.ERROR) console.error(this._format(LogLevel.ERROR, msg), ...args);
  }

  child(prefix) {
    return new Logger(this._level, this._prefix ? `${this._prefix}:${prefix}` : prefix);
  }
}
