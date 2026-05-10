function add(a, b) {
  return a + b;
}
function multiply(a, b) {
  return a * b;
}
function capitalize(str) {
  if (!str) return "";
  return str.charAt(0).toUpperCase() + str.slice(1);
}
function formatDate(isoString) {
  const date = new Date(isoString);
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}
function truncate(str, maxLen) {
  if (str.length <= maxLen) return str;
  return str.slice(0, maxLen - 3) + "...";
}
const LogLevel = Object.freeze({
  DEBUG: 0,
  INFO: 1,
  WARN: 2,
  ERROR: 3
});
class Logger {
  constructor(level = LogLevel.INFO, prefix = "") {
    this._level = level;
    this._prefix = prefix;
  }
  _format(level, message) {
    const ts = (/* @__PURE__ */ new Date()).toISOString();
    const name = Object.keys(LogLevel).find((k) => LogLevel[k] === level) || "UNKNOWN";
    const parts = [ts, `[${name}]`];
    if (this._prefix) parts.push(`(${this._prefix})`);
    parts.push(message);
    return parts.join(" ");
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
const _data = /* @__PURE__ */ new WeakMap();
const _subs = /* @__PURE__ */ new WeakMap();
const CHANGE = Symbol("change");
const RESET = Symbol("reset");
class Store {
  constructor(initial = {}) {
    _data.set(this, { ...initial });
    _subs.set(this, []);
  }
  get(key) {
    const data = _data.get(this);
    return data ? data[key] : void 0;
  }
  set(key, value) {
    const data = _data.get(this);
    const old = data[key];
    data[key] = value;
    if (old !== value) {
      this._notify(CHANGE, { key, old, value });
    }
  }
  reset(initial = {}) {
    _data.set(this, { ...initial });
    this._notify(RESET, initial);
  }
  get size() {
    return Object.keys(_data.get(this)).length;
  }
  subscribe(fn) {
    const subs = _subs.get(this);
    subs.push(fn);
    return () => {
      const idx = subs.indexOf(fn);
      if (idx !== -1) subs.splice(idx, 1);
    };
  }
  _notify(type, payload) {
    for (const fn of _subs.get(this)) {
      fn(type, payload);
    }
  }
}
const BASE_URL = "https://api.example.com";
function request(path) {
  return fetch(`${BASE_URL}${path}`).then((res) => {
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return res.json();
  });
}
function getUser(id) {
  return request(`/users/${id}`);
}
function getPosts(userId) {
  return request(`/users/${userId}/posts`);
}
const log = new Logger(LogLevel.INFO);
async function main() {
  log.info("Starting app");
  const user = await getUser(1);
  log.info(`User: ${capitalize(user.name)}`);
  const posts = await getPosts(user.id);
  const summary = posts.map((p) => ({
    title: truncate(capitalize(p.title), 50),
    date: formatDate(p.createdAt || (/* @__PURE__ */ new Date()).toISOString()),
    wordCount: p.body ? p.body.split(" ").length : 0
  }));
  const totalWords = summary.reduce((sum, p) => add(sum, p.wordCount), 0);
  const avg = Math.round(multiply(totalWords, 1 / summary.length));
  log.info(`${summary.length} posts, avg ${avg} words`);
  const store = new Store({ count: 0, user: user.name });
  store.subscribe((type, payload) => {
    if (type === CHANGE) {
      log.debug(`${payload.key} = ${payload.value}`);
    }
  });
  store.set("count", totalWords);
  return { user, summary, totalWords, avg };
}
main().then((result) => console.log("Done:", result));
export {
  main
};
