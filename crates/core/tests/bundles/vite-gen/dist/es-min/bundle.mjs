function _(n, t) {
  return n + t;
}
function m(n, t) {
  return n * t;
}
function g(n) {
  return n ? n.charAt(0).toUpperCase() + n.slice(1) : "";
}
function w(n) {
  const t = new Date(n), e = t.getFullYear(), s = String(t.getMonth() + 1).padStart(2, "0"), i = String(t.getDate()).padStart(2, "0");
  return `${e}-${s}-${i}`;
}
function $(n, t) {
  return n.length <= t ? n : n.slice(0, t - 3) + "...";
}
const o = Object.freeze({
  DEBUG: 0,
  INFO: 1,
  WARN: 2,
  ERROR: 3
});
class l {
  constructor(t = o.INFO, e = "") {
    this._level = t, this._prefix = e;
  }
  _format(t, e) {
    const s = (/* @__PURE__ */ new Date()).toISOString(), i = Object.keys(o).find((r) => o[r] === t) || "UNKNOWN", c = [s, `[${i}]`];
    return this._prefix && c.push(`(${this._prefix})`), c.push(e), c.join(" ");
  }
  debug(t, ...e) {
    this._level <= o.DEBUG && console.debug(this._format(o.DEBUG, t), ...e);
  }
  info(t, ...e) {
    this._level <= o.INFO && console.info(this._format(o.INFO, t), ...e);
  }
  warn(t, ...e) {
    this._level <= o.WARN && console.warn(this._format(o.WARN, t), ...e);
  }
  error(t, ...e) {
    this._level <= o.ERROR && console.error(this._format(o.ERROR, t), ...e);
  }
  child(t) {
    return new l(this._level, this._prefix ? `${this._prefix}:${t}` : t);
  }
}
const a = /* @__PURE__ */ new WeakMap(), h = /* @__PURE__ */ new WeakMap(), d = Symbol("change"), b = Symbol("reset");
class O {
  constructor(t = {}) {
    a.set(this, { ...t }), h.set(this, []);
  }
  get(t) {
    const e = a.get(this);
    return e ? e[t] : void 0;
  }
  set(t, e) {
    const s = a.get(this), i = s[t];
    s[t] = e, i !== e && this._notify(d, { key: t, old: i, value: e });
  }
  reset(t = {}) {
    a.set(this, { ...t }), this._notify(b, t);
  }
  get size() {
    return Object.keys(a.get(this)).length;
  }
  subscribe(t) {
    const e = h.get(this);
    return e.push(t), () => {
      const s = e.indexOf(t);
      s !== -1 && e.splice(s, 1);
    };
  }
  _notify(t, e) {
    for (const s of h.get(this))
      s(t, e);
  }
}
const R = "https://api.example.com";
function p(n) {
  return fetch(`${R}${n}`).then((t) => {
    if (!t.ok) throw new Error(`HTTP ${t.status}`);
    return t.json();
  });
}
function S(n) {
  return p(`/users/${n}`);
}
function y(n) {
  return p(`/users/${n}/posts`);
}
const f = new l(o.INFO);
async function N() {
  f.info("Starting app");
  const n = await S(1);
  f.info(`User: ${g(n.name)}`);
  const e = (await y(n.id)).map((r) => ({
    title: $(g(r.title), 50),
    date: w(r.createdAt || (/* @__PURE__ */ new Date()).toISOString()),
    wordCount: r.body ? r.body.split(" ").length : 0
  })), s = e.reduce((r, u) => _(r, u.wordCount), 0), i = Math.round(m(s, 1 / e.length));
  f.info(`${e.length} posts, avg ${i} words`);
  const c = new O({ count: 0, user: n.name });
  return c.subscribe((r, u) => {
    r === d && f.debug(`${u.key} = ${u.value}`);
  }), c.set("count", s), { user: n, summary: e, totalWords: s, avg: i };
}
N().then((n) => console.log("Done:", n));
export {
  N as main
};
