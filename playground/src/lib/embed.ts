/**
 * `?embed=1` hides the header and controls so the playground can sit inside
 * an iframe (the docs site uses it). Settings still come from the share hash.
 */
export function readEmbedFlag(search: string): boolean {
  return new URLSearchParams(search).get("embed") === "1";
}

/** The same playground state as a standalone page: drop the embed flag. */
export function standaloneUrl(href: string): string {
  const url = new URL(href);
  url.searchParams.delete("embed");
  return url.toString();
}
