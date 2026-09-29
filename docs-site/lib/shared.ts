export const appName = 'Wakaru';
// Docs pages live at the app root; the public /docs prefix comes from
// `basePath` in next.config.mjs (the site proxies wakarujs.com/docs here).
export const docsRoute = '/';
export const docsPublicBasePath = '/docs';
// The public origin. Canonical and sitemap URLs point here, never at the
// wakaru-docs.vercel.app deployment that serves the same pages.
export const docsSiteOrigin = 'https://wakarujs.com';
export const docsImageRoute = '/og/docs';
export const docsContentRoute = '/llms.mdx/docs';

export const gitConfig = {
  user: 'pionxzh',
  repo: 'wakaru',
  branch: 'main',
};
