declare interface Window {
  ipc: {
    postMessage(message: string): void;
  };
  /** Set when the app is ready in budget-mode (?budget=1) */
  __APP_READY?: boolean;
}

/**
 * Build-time constant (vite.config.ts `define`): true in dev and e2e builds,
 * false in production builds. Guards the `window.__e2e` test hook.
 */
declare const __CFC_TEST_HOOKS__: boolean;
