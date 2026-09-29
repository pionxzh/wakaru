import type { MetadataRoute } from 'next';
import { source } from '@/lib/source';
import { docsCanonicalUrl } from '@/lib/public-path';

// Served at /docs/sitemap.xml (basePath applies). The main site's robots.txt
// lists it next to the hand-written landing sitemap.
export default function sitemap(): MetadataRoute.Sitemap {
  return source.getPages().map((page) => ({
    url: docsCanonicalUrl(page.url),
  }));
}
