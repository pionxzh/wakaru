import { NextRequest, NextResponse } from 'next/server';
import { isMarkdownPreferred, rewritePath } from 'fumadocs-core/negotiation';
import { docsContentRoute } from '@/lib/shared';

// Docs pages live at the app root (see lib/shared.ts), so these patterns
// must not swallow internal routes.
const INTERNAL_PREFIXES = ['/api/', '/og/', '/llms.', '/llms-', '/_next/'];

const { rewrite: rewriteDocs } = rewritePath(
  '{/*path}',
  `${docsContentRoute}{/*path}/content.md`,
);
const { rewrite: rewriteSuffix } = rewritePath(
  '{/*path}.md',
  `${docsContentRoute}{/*path}/content.md`,
);

export default function proxy(request: NextRequest) {
  const { pathname } = request.nextUrl;
  if (INTERNAL_PREFIXES.some((prefix) => pathname.startsWith(prefix))) {
    return NextResponse.next();
  }

  // `nextUrl.pathname` has basePath stripped, but rewrite targets need it back.
  const { basePath } = request.nextUrl;

  const result = rewriteSuffix(pathname);
  if (result) {
    return NextResponse.rewrite(new URL(basePath + result, request.nextUrl));
  }

  if (isMarkdownPreferred(request)) {
    const result = rewriteDocs(pathname);

    if (result) {
      return NextResponse.rewrite(new URL(basePath + result, request.nextUrl));
    }
  }

  return NextResponse.next();
}
