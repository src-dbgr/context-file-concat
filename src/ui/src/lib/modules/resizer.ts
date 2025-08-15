import { elements } from "../dom.js";

export function setupResizerListeners() {
  // --- Vertical Resizer using Pointer Events API ---
  elements.resizer.addEventListener("pointerdown", (e) => {
    // Prevent default browser actions like text selection during drag.
    e.preventDefault();

    // Capture the pointer to ensure all subsequent pointer events are
    // retargeted to this element until the pointer is released.
    elements.resizer.setPointerCapture(e.pointerId);

    document.body.classList.add("vertical-resizing");

    const handlePointerMove = (moveEvent) => {
      const totalHeight = elements.contentSplitter.offsetHeight;
      // Ensure clientY is used for consistent coordinates regardless of scroll.
      const newTopHeight = moveEvent.clientY - elements.fileListPanel.offsetTop;

      // Set min/max boundaries for the resize operation.
      const minHeight = 100; // px
      const maxHeight = totalHeight - 100; // px

      if (newTopHeight > minHeight && newTopHeight < maxHeight) {
        const newTopPercent = (newTopHeight / totalHeight) * 100;
        elements.fileListPanel.style.height = `${newTopPercent}%`;
        elements.previewPanel.style.height = `${100 - newTopPercent}%`;
      }
    };

    const handlePointerUp = () => {
      // Crucial: Release the pointer capture to allow normal event flow.
      elements.resizer.releasePointerCapture(e.pointerId);
      document.body.classList.remove("vertical-resizing");

      // Clean up global event listeners to prevent memory leaks.
      document.removeEventListener("pointermove", handlePointerMove);
      document.removeEventListener("pointerup", handlePointerUp);
    };

    // Attach listeners to the document for the duration of the drag.
    document.addEventListener("pointermove", handlePointerMove);
    document.addEventListener("pointerup", handlePointerUp);
  });

  // --- Sidebar Resizer (Legacy Mouse Events) ---
  // Note: This could also be migrated to Pointer Events for consistency.
  let isSidebarDrag = false;
  let startX = 0;
  let startWidth = 0;
  const sidebar = document.querySelector(".sidebar");

  const handleSidebarDown = (e) => {
    if (!sidebar) return;
    const rect = sidebar.getBoundingClientRect();

    // Activate resizer only when the mouse is within a small threshold of the edge.
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

  const handleSidebarMove = (e) => {
    if (isSidebarDrag) {
      // Calculate new width within defined constraints.
      const newWidth = Math.max(
        280,
        Math.min(600, startWidth + e.clientX - startX)
      );
      sidebar.style.width = `${newWidth}px`;
    } else if (sidebar) {
      // Update cursor style on hover over the resize handle area.
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
