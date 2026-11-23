<script lang="ts">
  /**
   * The one root of the UI: the page layout with every panel in its place.
   * The element ids and classes are the ones the global stylesheet and the
   * resizers work with.
   */
  import { verticalResizer, sidebarResizer } from "$lib/actions/resizer";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import Header from "$lib/components/Header.svelte";
  import FileTree from "$lib/components/FileTree.svelte";
  import PreviewPanel from "$lib/components/PreviewPanel.svelte";
  import Footer from "$lib/components/Footer.svelte";
  import StatusBar from "$lib/components/StatusBar.svelte";
  import ToastHost from "$lib/components/ToastHost.svelte";
</script>

<div class="container" style="height: calc(100% - 30px)">
  <aside id="sidebar-root" class="sidebar" use:sidebarResizer>
    <Sidebar />
  </aside>

  <main class="main-content">
    <div class="sidebar-resize-handle-right" aria-hidden="true"></div>

    <div id="header-root">
      <Header />
    </div>

    <div class="content-splitter">
      <div id="file-list-panel" class="file-list-panel">
        <div id="file-tree-container" class="file-tree-container">
          <FileTree />
        </div>
      </div>

      <div id="resizer" class="resizer" use:verticalResizer></div>

      <div id="preview-panel" class="preview-panel">
        <PreviewPanel />
      </div>
    </div>

    <footer id="footer-root" class="bottom-panel">
      <Footer />
    </footer>
  </main>
</div>

<StatusBar />
<ToastHost />
