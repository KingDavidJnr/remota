// Normalized control event protocol (§14)

export type ControlMessage =
  | { type: "mouse_move"; x: number; y: number }
  | { type: "mouse_button"; action: "down" | "up"; button: "left" | "right" | "middle" }
  | { type: "mouse_dblclick"; x: number; y: number }
  | { type: "scroll"; deltaX: number; deltaY: number }
  | { type: "keyboard"; action: "down" | "up"; key: string };

/**
 * Convert a PointerEvent position inside a <video> element
 * to normalized [0, 1] coordinates relative to the video frame.
 */
export function normalizePointer(
  e: React.PointerEvent<HTMLVideoElement>
): { x: number; y: number } {
  const rect = e.currentTarget.getBoundingClientRect();
  return {
    x: (e.clientX - rect.left) / rect.width,
    y: (e.clientY - rect.top) / rect.height,
  };
}
