import { source } from '@/lib/source';
import { rewriteDocsMarkdownLinks } from '@/lib/public-path';
import { llms } from 'fumadocs-core/source';

export const revalidate = false;

export function GET() {
  return new Response(rewriteDocsMarkdownLinks(llms(source).index()));
}
