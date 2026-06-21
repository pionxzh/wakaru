// ESM entry importing CJS modules — forces interop helpers
import { add, multiply } from './math.cjs';
import { capitalize, truncate } from './format.cjs';
import Logger from './logger.cjs';
import { Store, CHANGE } from './store.cjs';

const log = new Logger('app: ');

export async function main() {
  log.info('Starting app');

  const items = [10, 20, 30, 40, 50];
  const total = items.reduce((sum, n) => add(sum, n), 0);
  const avg = multiply(total, 1 / items.length);
  const label = truncate(capitalize('average value'), 10);
  log.info(label + ': ' + avg);

  const store = new Store({ count: 0 });
  store.subscribe(function(type, payload) {
    if (type === CHANGE) {
      log.info(payload.key + ' = ' + payload.value);
    }
  });
  store.set('count', total);

  return { total, avg, label };
}
