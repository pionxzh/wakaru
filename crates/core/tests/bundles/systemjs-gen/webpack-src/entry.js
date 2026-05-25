import double, { named } from "./dep.js";

export const value = double(named);

export default function run() {
  return value + 1;
}
