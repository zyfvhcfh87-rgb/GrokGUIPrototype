import { useRef } from "react";

import { clampPanelWidth } from "../application/presentation.ts";

export function PanelResize({
  label,
  value,
  invert = false,
  cssVariable,
  onChange,
}: {
  label: string;
  value: number;
  invert?: boolean;
  cssVariable: "--projects-width" | "--details-width";
  onChange: (width: number) => void;
}) {
  const origin = useRef({ x: 0, width: value });
  const live = useRef(value);

  const preview = (next: number, node: HTMLElement) => {
    live.current = next;
    node.parentElement?.style.setProperty(cssVariable, `${next}px`);
    node.setAttribute("aria-valuenow", String(next));
  };

  return (
    <div
      className="panel-resize"
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuemin={200}
      aria-valuemax={420}
      aria-valuenow={value}
      tabIndex={0}
      onPointerDown={(event) => {
        const handle = event.currentTarget;
        origin.current = { x: event.clientX, width: value };
        live.current = value;
        const handleMove = (move: PointerEvent) => {
          const delta = move.clientX - origin.current.x;
          const next = invert ? origin.current.width - delta : origin.current.width + delta;
          preview(clampPanelWidth(next), handle);
        };
        const handleUp = () => {
          window.removeEventListener("pointermove", handleMove);
          window.removeEventListener("pointerup", handleUp);
          onChange(live.current);
        };
        window.addEventListener("pointermove", handleMove);
        window.addEventListener("pointerup", handleUp);
      }}
      onKeyDown={(event) => {
        if (event.key === "ArrowLeft") {
          event.preventDefault();
          onChange(clampPanelWidth(value + (invert ? 16 : -16)));
        }
        if (event.key === "ArrowRight") {
          event.preventDefault();
          onChange(clampPanelWidth(value + (invert ? -16 : 16)));
        }
        if (event.key === "Home") {
          event.preventDefault();
          onChange(200);
        }
        if (event.key === "End") {
          event.preventDefault();
          onChange(420);
        }
      }}
    />
  );
}
