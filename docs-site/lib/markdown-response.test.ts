import { describe, expect, it } from 'vitest';
import { markdownResponse } from './markdown-response';

describe('markdownResponse', () => {
  it('marks negotiated Markdown responses with their cache key', async () => {
    const response = markdownResponse('# Page');

    expect(response.headers.get('Content-Type')).toBe('text/markdown; charset=utf-8');
    expect(response.headers.get('Vary')).toBe('Accept');
    expect(await response.text()).toBe('# Page');
  });
});
