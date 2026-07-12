export type OutputView = "javascript" | "vue";

export interface VuePreviewState {
  sfc: string | null;
  view: OutputView;
}

export function applyVuePreviewResult(
  state: VuePreviewState,
  sfc: string | null
): VuePreviewState {
  return {
    sfc,
    view: sfc ? state.view : "javascript",
  };
}

export function resetVuePreview(enabled: boolean): VuePreviewState {
  return {
    sfc: null,
    view: enabled ? "vue" : "javascript",
  };
}
