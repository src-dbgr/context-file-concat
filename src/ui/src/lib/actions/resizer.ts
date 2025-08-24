// Svelte-/imperative Actions für Resizing (vertikal zwischen Filelist/Preview
// und horizontal für die Sidebar-Breite).

/**
 * Vertical splitter between File-Panel and Preview.
 * Is bound in `main.ts` directly onto #resizer.
 */
export function verticalResizer(node: HTMLElement) {
  const contentSplitter = node.closest(".content-splitter");
  const fileListPanel = document.getElementById("file-list-panel");
  const previewPanel = document.getElementById("preview-panel");

  function onPointerDown(e: PointerEvent) {
    if (
      !(fileListPanel instanceof HTMLElement) ||
      !(previewPanel instanceof HTMLElement) ||
      !(contentSplitter instanceof HTMLElement)
    ) {
      return;
    }

    e.preventDefault();
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
        // notify listeners (FileTree measures viewport on resize)
        window.dispatchEvent(new CustomEvent("cfc:layout"));
      }
    };

    const onUp = () => {
      document.body.classList.remove("vertical-resizing");
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
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
 * Bound in `main.ts` to `<aside.sidebar>`.
 *
 * Zone-widht comes from CSS-Variable --resize-zone (px).
 */
export function sidebarResizer(sidebarEl: HTMLElement) {
  const MIN = 280;
  const MAX = 600;

  function getZone(): number {
    const raw = getComputedStyle(document.documentElement)
      .getPropertyValue("--resize-zone")
      .trim();
    const num = parseFloat(raw);
    return Number.isFinite(num) ? num : 12;
  }

  let dragging = false;
  let startX = 0;
  let startWidth = 0;

  function inZone(e: PointerEvent): boolean {
    const zone = getZone();
    const r = sidebarEl.getBoundingClientRect();
    return (
      e.clientX >= r.right - zone &&
      e.clientX <= r.right + zone &&
      e.clientY >= r.top &&
      e.clientY <= r.bottom
    );
  }

  function onPointerDown(e: PointerEvent) {
    if (!inZone(e)) return;
    dragging = true;
    startX = e.clientX;
    startWidth = sidebarEl.getBoundingClientRect().width;
    document.body.classList.add("sidebar-resizing");
    e.preventDefault();
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging) return;
    const w = Math.max(MIN, Math.min(MAX, startWidth + (e.clientX - startX)));
    sidebarEl.style.width = `${w}px`;
  }

  function onPointerUp() {
    if (!dragging) return;
    dragging = false;
    document.body.classList.remove("sidebar-resizing");
  }

  window.addEventListener("pointerdown", onPointerDown);
  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", onPointerUp);
  window.addEventListener("pointercancel", onPointerUp);

  return {
    destroy() {
      window.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      window.removeEventListener("pointercancel", onPointerUp);
    },
  };
}
