import { useEffect, useRef } from "react";
import { useStore, type ReactFlowState } from "@xyflow/react";

export interface HelperLineOverlayProps {
  horizontal?: number;
  vertical?: number;
}

const selector = (s: ReactFlowState) => ({
  width: s.width,
  height: s.height,
  transform: s.transform,
});

// Draws alignment guide lines (computed in flow space) onto a canvas overlay,
// mapping flow coordinates through the current viewport transform.
export function HelperLineOverlay({ horizontal, vertical }: HelperLineOverlayProps) {
  const { width, height, transform } = useStore(selector);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;

    const dpr = window.devicePixelRatio || 1;
    canvas.width = width * dpr;
    canvas.height = height * dpr;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, width, height);
    ctx.strokeStyle = "#3b82f6";
    ctx.lineWidth = 1;

    const [tx, ty, scale] = transform;
    if (typeof vertical === "number") {
      const x = vertical * scale + tx;
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, height);
      ctx.stroke();
    }
    if (typeof horizontal === "number") {
      const y = horizontal * scale + ty;
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(width, y);
      ctx.stroke();
    }
  }, [width, height, transform, horizontal, vertical]);

  return (
    <canvas
      ref={canvasRef}
      className="helper-lines"
      style={{
        width,
        height,
        position: "absolute",
        top: 0,
        left: 0,
        zIndex: 10,
        pointerEvents: "none",
      }}
    />
  );
}
