import { elements } from "../dom.js";

export function setupResizerListeners() {
  elements.resizer.addEventListener("pointerdown", (e: PointerEvent) => {
    e.preventDefault();
    elements.resizer.setPointerCapture(e.pointerId);
    document.body.classList.add("vertical-resizing");

    const handlePointerMove = (moveEvent: PointerEvent) => {
      const totalHeight = elements.contentSplitter.offsetHeight;
      const newTopHeight = moveEvent.clientY - elements.fileListPanel.offsetTop;
      const minHeight = 100;
      const maxHeight = totalHeight - 100;

      if (newTopHeight > minHeight && newTopHeight < maxHeight) {
        const newTopPercent = (newTopHeight / totalHeight) * 100;
        elements.fileListPanel.style.height = `${newTopPercent}%`;
        elements.previewPanel.style.height = `${100 - newTopPercent}%`;
      }
    };

    const handlePointerUp = () => {
      elements.resizer.releasePointerCapture(e.pointerId);
      document.body.classList.remove("vertical-resizing");
      document.removeEventListener("pointermove", handlePointerMove);
      document.removeEventListener("pointerup", handlePointerUp);
    };

    document.addEventListener("pointermove", handlePointerMove);
    document.addEventListener("pointerup", handlePointerUp);
  });

  let isSidebarDrag = false;
  let startX = 0;
  let startWidth = 0;
  const sidebar = document.querySelector<HTMLElement>(".sidebar");

  const handleSidebarDown = (e: MouseEvent) => {
    if (!sidebar) return;
    const rect = sidebar.getBoundingClientRect();

    if (e.clientX >= rect.right - 5 && e.clientX <= rect.right + 5) {
      e.preventDefault();
      isSidebarDrag = true;
      startX = e.clientX;
      startWidth = parseInt(getComputedStyle(sidebar).width, 10);
      document.body.classList.add("sidebar-resizing");
    }
  };

  const handleSidebarUp = () => {
    if (isSidebarDrag) {
      isSidebarDrag = false;
      document.body.classList.remove("sidebar-resizing");
      document.body.style.cursor = "default";
    }
  };

  const handleSidebarMove = (e: MouseEvent) => {
    if (!sidebar) return;

    if (isSidebarDrag) {
      const newWidth = Math.max(
        280,
        Math.min(600, startWidth + e.clientX - startX)
      );
      sidebar.style.width = `${newWidth}px`;
    } else {
      const rect = sidebar.getBoundingClientRect();
      if (e.clientX >= rect.right - 5 && e.clientX <= rect.right + 5) {
        document.body.style.cursor = "ew-resize";
      } else if (document.body.style.cursor === "ew-resize") {
        document.body.style.cursor = "default";
      }
    }
  };

  document.addEventListener("mousedown", handleSidebarDown);
  document.addEventListener("mouseup", handleSidebarUp);
  document.addEventListener("mousemove", handleSidebarMove);
}
