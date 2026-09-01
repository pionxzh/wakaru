import type { OutputView } from "./vuePreview";

export type OutputPaneView = OutputView | "diff";

export interface OutputPaneOptions {
  diffRequested: boolean;
  diffAvailable: boolean;
  vueRequested: boolean;
  vueAvailable: boolean;
}

export function resolveOutputPaneView(options: OutputPaneOptions): OutputPaneView {
  if (options.diffRequested && options.diffAvailable) {
    return "diff";
  }
  if (options.vueRequested && options.vueAvailable) {
    return "vue";
  }
  return "javascript";
}
