declare interface Window {
  ipc: {
    postMessage(message: string): void;
  };
  /** Set by Playwright tests to enable the E2E bridge in production preview */
  __PW_E2E?: boolean;
  /** Set when the app is ready in budget-mode (?budget=1) */
  __APP_READY?: boolean;
}
