// original comment: keep me
export interface Pair { left: number; right: number }
export function addPair(pair: Pair): number {
  const { left, right } = pair;
  return left + right;
}
