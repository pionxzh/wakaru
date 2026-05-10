import { version } from './version';

console.log(version);

async function loadGreet() {
  const mod = await import('./greet');
  return mod.greet('dynamic');
}

loadGreet().then(console.log);
