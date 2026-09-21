import { useEffect, useRef } from "react";

import { clampPanelWidth, createPanelWidthWriter } from "../application/presentation.ts";

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
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const writer = useRef(createPanelWidthWriter((next) => onChangeRef.current(next)));

  useEffect(() => {
    live.current = value;
  }, [value]);

  useEffect(() => {
    const current = writer.current;
    return () => {
      current.dispose();
    };
  }, []);

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
        origin.current = { x: event.clientX, width: live.current };
        const handleMove = (move: PointerEvent) => {
          const delta = move.clientX - origin.current.x;
          const next = invert ? origin.current.width - delta : origin.current.width + delta;
          preview(clampPanelWidth(next), handle);
        };
        const handleUp = () => {
          window.removeEventListener("pointermove", handleMove);
          window.removeEventListener("pointerup", handleUp);
          writer.current.writePointer(live.current);
        };
        window.addEventListener("pointermove", handleMove);
        window.addEventListener("pointerup", handleUp);
      }}
      onKeyDown={(event) => {
        const handle = event.currentTarget;
        const step = (delta: number) => {
          event.preventDefault();
          preview(clampPanelWidth(live.current + delta), handle);
          writer.current.writeKeyboard(live.current);
        };
        if (event.key === "ArrowLeft") {
          step(invert ? 16 : -16);
        }
        if (event.key === "ArrowRight") {
          step(invert ? -16 : 16);
        }
        if (event.key === "Home") {
          event.preventDefault();
          preview(200, handle);
          writer.current.writeKeyboard(200);
        }
        if (event.key === "End") {
          event.preventDefault();
          preview(420, handle);
          writer.current.writeKeyboard(420);
        }
      }}
    />
  );
}
