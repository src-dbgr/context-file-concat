// Svelte-/imperative Actions für Resizing (vertikal zwischen Filelist/Preview
// und horizontal für die Sidebar-Breite).

/**
 * Vertical splitter between File-Panel and Preview.
 * Is bound in `main.ts` directly onto #resizer.
 */
export function verticalResizer(node: HTMLElement) {
  const contentSplitter = node.closest(
    ".content-splitter"
  ) as HTMLElement | null;
  const fileListPanel = document.getElementById(
    "file-list-panel"
  ) as HTMLElement | null;
  const previewPanel = document.getElementById(
    "preview-panel"
  ) as HTMLElement | null;

  function onPointerDown(e: PointerEvent) {
    if (!fileListPanel || !previewPanel || !contentSplitter) return;

    e.preventDefault();
    node.setPointerCapture(e.pointerId);
    document.body.classList.add("vertical-resizing");

    const splitterTop = contentSplitter.getBoundingClientRect().top;

    const onMove = (evt: PointerEvent) => {
      const totalHeight = contentSplitter.offsetHeight;
      const newTopHeight = evt.clientY - splitterTop;
      const minHeight = 100;
      const maxHeight = totalHeight - 100;

      if (newTopHeight > minHeight && newTopHeight < maxHeight) {
        const newTopPercent = (newTopHeight / totalHeight) * 100;
        fileListPanel.style.height = `${newTopPercent}%`;
        previewPanel.style.height = `${100 - newTopPercent}%`;
      }
    };

    const onUp = () => {
      try {
        node.releasePointerCapture(e.pointerId);
      } catch {}
      document.body.classList.remove("vertical-resizing");
      document.removeEventListener("pointermove", onMove);
      document.removeEventListener("pointerup", onUp);
    };

    document.addEventListener("pointermove", onMove);
    document.addEventListener("pointerup", onUp);
  }

  node.addEventListener("pointerdown", onPointerDown);
  return {
    destroy() {
      node.removeEventListener("pointerdown", onPointerDown);
    },
  };
}

/**
 * Sidebar-Resizer: expects the **concrete Sidebar-HTMLElement-reference**.
 * In bound in `main.ts` after mount to `<aside.sidebar>`.
 */
export function sidebarResizer(sidebarEl: HTMLElement) {
  let isDrag = false;
  let startX = 0;
  let startWidth = 0;

  function isNearRightEdge(x: number) {
    const rect = sidebarEl.getBoundingClientRect();
    return x >= rect.right - 5 && x <= rect.right + 5;
  }

  function onMouseDown(e: MouseEvent) {
    if (!isNearRightEdge(e.clientX)) return;
    e.preventDefault();
    isDrag = true;
    startX = e.clientX;
    startWidth = parseInt(getComputedStyle(sidebarEl).width, 10);
    document.body.classList.add("sidebar-resizing");
  }

  function onMouseMove(e: MouseEvent) {
    if (isDrag) {
      const newWidth = Math.max(
        280,
        Math.min(600, startWidth + e.clientX - startX)
      );
      sidebarEl.style.width = `${newWidth}px`;
    } else {
      if (isNearRightEdge(e.clientX)) {
        document.body.style.cursor = "ew-resize";
      } else if (document.body.style.cursor === "ew-resize") {
        document.body.style.cursor = "default";
      }
    }
  }

  function onMouseUp() {
    if (isDrag) {
      isDrag = false;
      document.body.classList.remove("sidebar-resizing");
      document.body.style.cursor = "default";
    }
  }

  document.addEventListener("mousedown", onMouseDown);
  document.addEventListener("mousemove", onMouseMove);
  document.addEventListener("mouseup", onMouseUp);

  return {
    destroy() {
      document.removeEventListener("mousedown", onMouseDown);
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
    },
  };
}
