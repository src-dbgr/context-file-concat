/// <reference types="svelte" />
/// <reference types="svelte/elements" />
/// <reference types="vite/client" />

declare module "*.svelte" {
  import type { SvelteComponent } from "svelte";
  const component: typeof SvelteComponent;
  export default component;
}

// Declare the ipc property that the WebView environment provides.
declare interface Window {
  ipc: {
    postMessage(message: string): void;
  };
}
