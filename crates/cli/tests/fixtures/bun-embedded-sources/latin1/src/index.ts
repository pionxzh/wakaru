import { addPair, type Pair } from "./util/math";
const greeting: string = "hello from original source";
function main(): void {
  const pair: Pair = { left: 2, right: 3 };
  console.log(greeting, addPair(pair));
}
main();
