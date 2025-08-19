declare interface Window {
  ipc: {
    postMessage(message: string): void;
  };
}
