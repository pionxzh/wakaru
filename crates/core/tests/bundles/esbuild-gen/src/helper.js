function normalize(arr) { return arr.map(x => x / Math.max(...arr)); }

export function total(arr) { return normalize(arr).reduce((a, b) => a + b, 0); }
export function average(arr) { return total(arr) / arr.length; }
