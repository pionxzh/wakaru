import { bump, count, makeLabel, Widget } from "./provider.js";

function run() {
  const before = count;
  bump();
  return [new Widget("consumer").label(), makeLabel("x"), before, count];
}

globalThis.concatCwSink = { run };
