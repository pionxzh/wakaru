export const Box = { read() { return Box.value; }, value: 1 };
export namespace Shop { export function make() { return "made"; } }
export enum Mode { A, B }
export let counter = 0;
export function bump() { counter += 1; return counter; }
