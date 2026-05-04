export function compute(a, b) {
  return normalize(a) + normalize(b);
}

function normalize(x) {
  return x / Math.abs(x) || 0;
}
