// Minimal Monaco-Stub for Tests
export class Selection {
  startLineNumber: number;
  startColumn: number;
  endLineNumber: number;
  endColumn: number;
  constructor(a = 0, b = 0, c = 0, d = 0) {
    this.startLineNumber = a;
    this.startColumn = b;
    this.endLineNumber = c;
    this.endColumn = d;
  }
}

export const editor = {
  create: () => ({}),
};
export const languages = {
  register: () => void 0,
};
