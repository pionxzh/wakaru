import { bump, count, Widget } from "./provider.js";
import { describe } from "./relay.js";

function run() {
  const before = count;
  bump();
  return [new Widget("consumer").label(), describe("y"), before, count];
}

globalThis.concatCwSink = { run };
