const _data = new WeakMap();
const _subs = new WeakMap();

export const CHANGE = Symbol('change');
export const RESET = Symbol('reset');

export class Store {
  constructor(initial = {}) {
    _data.set(this, { ...initial });
    _subs.set(this, []);
  }

  get(key) {
    const data = _data.get(this);
    return data ? data[key] : undefined;
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
