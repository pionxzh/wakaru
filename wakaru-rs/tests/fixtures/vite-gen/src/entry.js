import { add, multiply } from './math.js';
import { capitalize, formatDate, truncate } from './format.js';
import { Logger, LogLevel } from './logger.js';
import { Store, CHANGE } from './store.js';
import { getUser, getPosts } from './api.js';

const log = new Logger(LogLevel.INFO);

async function main() {
  log.info('Starting app');

  const user = await getUser(1);
  log.info(`User: ${capitalize(user.name)}`);

  const posts = await getPosts(user.id);
  const summary = posts.map((p) => ({
    title: truncate(capitalize(p.title), 50),
    date: formatDate(p.createdAt || new Date().toISOString()),
    wordCount: p.body ? p.body.split(' ').length : 0,
  }));

  const totalWords = summary.reduce((sum, p) => add(sum, p.wordCount), 0);
  const avg = Math.round(multiply(totalWords, 1 / summary.length));
  log.info(`${summary.length} posts, avg ${avg} words`);

  const store = new Store({ count: 0, user: user.name });
  store.subscribe((type, payload) => {
    if (type === CHANGE) {
      log.debug(`${payload.key} = ${payload.value}`);
    }
  });
  store.set('count', totalWords);

  return { user, summary, totalWords, avg };
}

main().then((result) => console.log('Done:', result));

export { main };
