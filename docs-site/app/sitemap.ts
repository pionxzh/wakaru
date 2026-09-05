import type { MetadataRoute } from 'next';
import { source } from '@/lib/source';
import { withDocsBasePath } from '@/lib/public-path';

const origin = 'https://wakarujs.com';

// Served at /docs/sitemap.xml (basePath applies). The main site's robots.txt
// lists it next to the hand-written landing sitemap.
export default function sitemap(): MetadataRoute.Sitemap {
  return source.getPages().map((page) => ({
    url: new URL(withDocsBasePath(page.url), origin).toString(),
  }));
}
