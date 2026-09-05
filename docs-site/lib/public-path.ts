import { docsPublicBasePath } from './shared';

export function withDocsBasePath(url: string): string {
  if (!url.startsWith('/') || url === docsPublicBasePath || url.startsWith(`${docsPublicBasePath}/`)) {
    return url;
  }

  return url === '/' ? docsPublicBasePath : `${docsPublicBasePath}${url}`;
}

function rewriteLineLinks(line: string): string {
  return line
    .replace(/(\]\()\/([^\s)]*)/g, (_match, prefix: string, path: string) => {
      return `${prefix}${withDocsBasePath(`/${path}`)}`;
    })
    .replace(
      /(\b(?:href|src)=)(['"])\/([^'"]*)\2/g,
      (_match, attribute: string, quote: string, path: string) => {
        return `${attribute}${quote}${withDocsBasePath(`/${path}`)}${quote}`;
      },
    );
}

export function rewriteDocsMarkdownLinks(markdown: string): string {
  let fence: { marker: string; length: number } | null = null;

  return markdown
    .split('\n')
    .map((line) => {
      const fenceMatch = /^\s{0,3}(`{3,}|~{3,})/.exec(line);
      if (fenceMatch) {
        const marker = fenceMatch[1][0];
        const length = fenceMatch[1].length;
        if (fence === null) {
          fence = { marker, length };
        } else if (fence.marker === marker && length >= fence.length) {
          fence = null;
        }
        return line;
      }

      return fence === null ? rewriteLineLinks(line) : line;
    })
    .join('\n');
}
