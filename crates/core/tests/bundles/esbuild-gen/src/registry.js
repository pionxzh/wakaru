const modules = {};
export function register(name, mod) { modules[name] = mod; }
export function lookup(name) { return modules[name]; }
register("self", { loaded: true });
