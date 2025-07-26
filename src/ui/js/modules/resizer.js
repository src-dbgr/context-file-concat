import { elements } from '../dom.js';

export function setupResizerListeners() {
    let verticalMouseDown = false;
    elements.resizer.addEventListener("mousedown", () => {
        verticalMouseDown = true;
        document.body.classList.add("vertical-resizing");
    });

    let sidebarMouseDown = false;
    let startX = 0;
    let startWidth = 0;
    const sidebar = document.querySelector(".sidebar");

    document.addEventListener("mousedown", (e) => {
        if (!sidebar) return;
        const rect = sidebar.getBoundingClientRect();
        if (e.clientX >= rect.right - 5 && e.clientX <= rect.right + 5) {
            sidebarMouseDown = true;
            startX = e.clientX;
            startWidth = parseInt(getComputedStyle(sidebar).width, 10);
            document.body.classList.add("sidebar-resizing");
            e.preventDefault();
        }
    });

    document.addEventListener("mouseup", () => {
        verticalMouseDown = false;
        sidebarMouseDown = false;
        document.body.classList.remove("vertical-resizing", "sidebar-resizing");
        document.body.style.cursor = "default";
    });

    document.addEventListener("mousemove", (e) => {
        if (sidebarMouseDown && sidebar) {
            const newWidth = Math.max(280, Math.min(600, startWidth + e.clientX - startX));
            sidebar.style.width = newWidth + "px";
            e.preventDefault();
            return;
        }

        if (verticalMouseDown) {
            const totalHeight = elements.contentSplitter.offsetHeight;
            const newTopHeight = e.clientY - elements.fileListPanel.offsetTop;
            if (newTopHeight > 100 && newTopHeight < totalHeight - 100) {
                const newTopPercent = (newTopHeight / totalHeight) * 100;
                elements.fileListPanel.style.height = `${newTopPercent}%`;
                elements.previewPanel.style.height = `${100 - newTopPercent}%`;
            }
            return;
        }

        // Set cursor for sidebar resize hover
        if (sidebar && !sidebarMouseDown) {
             const rect = sidebar.getBoundingClientRect();
             if (e.clientX >= rect.right - 5 && e.clientX <= rect.right + 5) {
                 document.body.style.cursor = "ew-resize";
             } else if (document.body.style.cursor === "ew-resize") {
                 document.body.style.cursor = "default";
             }
        }
    });
}
