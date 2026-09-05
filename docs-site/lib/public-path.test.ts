import { describe, expect, it } from 'vitest';
import { rewriteDocsMarkdownLinks, withDocsBasePath } from './public-path';

describe('withDocsBasePath', () => {
  it('maps app-root paths to their public docs URLs', () => {
    expect(withDocsBasePath('/')).toBe('/docs');
    expect(withDocsBasePath('/guides/source-maps')).toBe('/docs/guides/source-maps');
  });

  it('leaves public docs, relative, and external URLs unchanged', () => {
    expect(withDocsBasePath('/docs/reference/cli')).toBe('/docs/reference/cli');
    expect(withDocsBasePath('guides/source-maps')).toBe('guides/source-maps');
    expect(withDocsBasePath('https://wakarujs.com/playground/')).toBe(
      'https://wakarujs.com/playground/',
    );
  });
});

describe('rewriteDocsMarkdownLinks', () => {
  it('prefixes Markdown and MDX links that target docs pages', () => {
    const markdown = `[Guide](/guides/source-maps)\n\n` +
      `<Card href="/reference/cli" />\n` +
      `<img src='/project/chart.png' />`;

    expect(rewriteDocsMarkdownLinks(markdown)).toBe(
      `[Guide](/docs/guides/source-maps)\n\n` +
        `<Card href="/docs/reference/cli" />\n` +
        `<img src='/docs/project/chart.png' />`,
    );
  });

  it('does not rewrite links that already include the prefix or leave the site', () => {
    const markdown = `[Docs](/docs/reference/cli)\n` +
      `[Playground](https://wakarujs.com/playground/)\n` +
      `[Relative](next-page)`;

    expect(rewriteDocsMarkdownLinks(markdown)).toBe(markdown);
  });

  it('does not alter examples inside fenced code blocks', () => {
    const markdown = `Before\n\n\`\`\`md\n[Example](/guides/example)\n\`\`\`\n\n` +
      `[Real](/guides/real)`;

    expect(rewriteDocsMarkdownLinks(markdown)).toBe(
      `Before\n\n\`\`\`md\n[Example](/guides/example)\n\`\`\`\n\n` +
        `[Real](/docs/guides/real)`,
    );
  });
});
