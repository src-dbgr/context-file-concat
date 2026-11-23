/* @vitest-environment jsdom */

// The file tree holds a window of the rows; the host owns the list. These
// tests cover what that asks of the component: reporting its viewport,
// rendering rows by their position in the whole list, and keyboard navigation
// and focus across the edge of the window.

import {
  describe,
  it,
  expect,
  beforeAll,
  afterAll,
  beforeEach,
  vi,
} from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import FileTree from "$lib/components/FileTree.svelte";
import { appState } from "$lib/stores/app";
import { ITEM_HEIGHT } from "$lib/modules/treeWindow";
import type { TreeRow } from "$lib/types";
import { dir, file, manyRows, rowsOf } from "./helpers/tree";
import { FakeTreeHost } from "./helpers/fakeHost";

vi.mock("$lib/services/backend", () => ({
  post: vi.fn(),
}));
import { post } from "$lib/services/backend";

/** The viewport of the tree in these tests: 10 rows. */
const VIEWPORT_ROWS = 10;

// jsdom does not lay anything out. Give the scroller of the tree a height and
// a scroll height, so that it can be measured and scrolled like in a browser.
const layout = ["clientHeight", "scrollHeight"] as const;
const original = layout.map((property) =>
  Object.getOwnPropertyDescriptor(HTMLElement.prototype, property),
);
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains("tree") ? VIEWPORT_ROWS * ITEM_HEIGHT : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get(this: HTMLElement) {
      const sizer = this.querySelector<HTMLElement>(".virtual-scroll-sizer");
      return sizer ? parseFloat(sizer.style.height) : 0;
    },
  });
});
afterAll(() => {
  layout.forEach((property, i) => {
    const descriptor = original[i];
    if (descriptor) {
      Object.defineProperty(HTMLElement.prototype, property, descriptor);
    } else {
      delete (HTMLElement.prototype as unknown as Record<string, unknown>)[
        property
      ];
    }
  });
});

/** Lets the deferred work of the tree (measuring, focus) run. */
const settle = async () => {
  for (let i = 0; i < 4; i++) {
    await tick();
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => resolve()),
    );
  }
  await tick();
};

function renderedIndices(container: HTMLElement): number[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(".tree-item-container"),
  ).map((el) => Number(el.getAttribute("data-index")));
}

function rowEl(container: HTMLElement, index: number): HTMLElement {
  const el = container.querySelector<HTMLElement>(
    `.tree-item-container[data-index="${index}"]`,
  );
  if (!el) throw new Error(`row ${index} is not rendered`);
  return el;
}

function focusedRow(container: HTMLElement): number | null {
  const el = container.querySelector('.tree-item-container[tabindex="0"]');
  return el ? Number(el.getAttribute("data-index")) : null;
}

async function mount(rows: TreeRow[]) {
  const host = new FakeTreeHost(rows);
  vi.mocked(post).mockImplementation(host.receive as typeof post);
  host.answer();
  const view = render(FileTree);
  const tree = await screen.findByRole("tree");
  await settle();
  return { host, tree, container: view.container };
}

async function scrollToRow(tree: HTMLElement, row: number) {
  tree.scrollTop = row * ITEM_HEIGHT;
  await fireEvent.scroll(tree);
  await tick();
}

/** The host answers everything the UI has asked for, until it asks no more. */
async function answerAll(host: FakeTreeHost) {
  for (let round = 0; round < 10; round++) {
    const before = host.received.length;
    host.answer();
    await settle();
    if (host.received.length === before) return;
  }
  throw new Error("the UI keeps asking");
}

beforeEach(async () => {
  vi.clearAllMocks();
  await settle();
  document.body.innerHTML = "";
  (document.activeElement as HTMLElement | null)?.blur?.();
});

describe("FileTree – the window of rows", () => {
  it("sizes the scroller by the whole list and renders the rows on screen", async () => {
    const { host, container } = await mount(manyRows(1000));

    const sizer = container.querySelector<HTMLElement>(
      ".virtual-scroll-sizer",
    )!;
    expect(sizer.style.height).toBe(`${1000 * ITEM_HEIGHT}px`);
    // 10 visible rows plus the overscan below.
    expect(renderedIndices(container)).toEqual(
      Array.from({ length: 15 }, (_, i) => i),
    );
    // The window the host sent at first is enough: nothing is asked for.
    expect(host.received).toEqual([]);
  });

  it("does not report the viewport while it stays inside the window", async () => {
    const { host, tree, container } = await mount(manyRows(1000));

    for (const row of [1, 5, 20, 40, 70]) {
      await scrollToRow(tree, row);
    }

    expect(host.received).toEqual([]);
    expect(renderedIndices(container)).toEqual(
      Array.from({ length: 20 }, (_, i) => 65 + i),
    );
    expect(
      rowEl(container, 70).closest<HTMLElement>(".virtual-scroll-item")!.style
        .top,
    ).toBe(`${70 * ITEM_HEIGHT}px`);
  });

  it("reports the viewport when it comes close to the edge of the window", async () => {
    const { host, tree } = await mount(manyRows(1000));

    await scrollToRow(tree, 80);

    // Rows 80..89 are visible; the UI asks for them with 30 rows around.
    expect(host.received).toEqual([["setViewport", { start: 50, count: 71 }]]);

    host.answer();
    await settle();
    expect(host.received).toHaveLength(1);
  });

  it("shows placeholders for rows that have not arrived, without moving anything", async () => {
    const { host, tree, container } = await mount(manyRows(1000));
    const placeholders = () =>
      Array.from(
        container.querySelectorAll<HTMLElement>(".virtual-scroll-item.pending"),
      );
    expect(placeholders()).toEqual([]);
    expect(tree).toHaveAttribute("aria-busy", "false");

    await scrollToRow(tree, 900);

    expect(host.posted("setViewport")).toEqual([{ start: 870, count: 71 }]);
    // No row of the window is on screen: every place holds a placeholder of
    // the height of a row, where the row will be.
    expect(renderedIndices(container)).toEqual([]);
    expect(placeholders().map((el) => el.style.top)).toEqual(
      Array.from({ length: 20 }, (_, i) => `${(895 + i) * ITEM_HEIGHT}px`),
    );
    for (const placeholder of placeholders()) {
      expect(placeholder.style.height).toBe(`${ITEM_HEIGHT}px`);
      // Nothing for assistive technology to read; the tree says it is busy.
      expect(placeholder.querySelector("[role]")).toBeNull();
      expect(placeholder.firstElementChild).toHaveAttribute(
        "aria-hidden",
        "true",
      );
    }
    expect(tree).toHaveAttribute("aria-busy", "true");
    // The scroller keeps its height: the scrollbar does not jump.
    expect(
      container.querySelector<HTMLElement>(".virtual-scroll-sizer")!.style
        .height,
    ).toBe(`${1000 * ITEM_HEIGHT}px`);

    host.answer();
    await settle();

    expect(placeholders()).toEqual([]);
    expect(tree).toHaveAttribute("aria-busy", "false");
    expect(renderedIndices(container)).toEqual(
      Array.from({ length: 20 }, (_, i) => 895 + i),
    );
    expect(
      rowEl(container, 900).closest<HTMLElement>(".virtual-scroll-item")!.style
        .top,
    ).toBe(`${900 * ITEM_HEIGHT}px`);
    expect(rowEl(container, 900)).toHaveAttribute(
      "data-path",
      "/repo/file-00900.txt",
    );
    expect(host.received).toHaveLength(1);
  });

  it("shows placeholders only for the part of the screen the window lacks", async () => {
    const { host, tree, container } = await mount(manyRows(1000));

    // The window holds rows 0..99: the last rows of this screen are missing.
    await scrollToRow(tree, 92);

    expect(renderedIndices(container)).toEqual([
      87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99,
    ]);
    expect(
      container.querySelectorAll(".virtual-scroll-item.pending"),
    ).toHaveLength(7);
    await answerAll(host);
    expect(
      container.querySelectorAll(".virtual-scroll-item.pending"),
    ).toHaveLength(0);
  });

  it("asks once per position, also when other snapshots arrive in between", async () => {
    const { host, tree } = await mount(manyRows(1000));

    await scrollToRow(tree, 900);
    // The same rows again (a scroll event by a pixel): the answer is under way.
    tree.scrollTop += 1;
    await fireEvent.scroll(tree);
    await tick();
    expect(host.posted("setViewport")).toHaveLength(1);

    // The status line changes; that is no new snapshot of the host.
    appState.update((s) => {
      s.status_message = "Status: something";
      return s;
    });
    await tick();
    expect(host.posted("setViewport")).toHaveLength(1);

    // Scrolling on before the answer is here: the viewport has moved.
    await scrollToRow(tree, 500);
    expect(host.posted("setViewport")).toEqual([
      { start: 870, count: 71 },
      { start: 470, count: 71 },
    ]);

    await answerAll(host);
    expect(host.posted("setViewport")).toHaveLength(2);
  });

  it("follows a list that has become shorter than the viewport position", async () => {
    const { host, tree, container } = await mount(manyRows(1000));
    await scrollToRow(tree, 900);
    await answerAll(host);

    // "Collapse all": 12 rows are left. The host moves its window to the end.
    host.setRows(manyRows(12));
    host.answer();
    await settle();
    await answerAll(host);

    expect(renderedIndices(container)).toEqual(
      Array.from({ length: 12 }, (_, i) => i),
    );
    expect(tree.scrollTop).toBeLessThanOrEqual(2 * ITEM_HEIGHT);
  });

  it("starts at the top again when another directory is loaded", async () => {
    const { host, tree, container } = await mount(manyRows(1000));
    await scrollToRow(tree, 900);
    await answerAll(host);

    // The host shows another directory from the top by itself.
    host.setRows(manyRows(400), { current_path: "/other" });
    host.received.length = 0;
    host.answer();
    await settle();
    await answerAll(host);

    expect(tree.scrollTop).toBe(0);
    expect(renderedIndices(container)[0]).toBe(0);
    expect(focusedRow(container)).toBe(0);
  });
});

describe("FileTree – keyboard across the edge of the window", () => {
  async function mountFocused(rows: TreeRow[]) {
    const mounted = await mount(rows);
    await waitFor(() => expect(focusedRow(mounted.container)).toBe(0));
    expect(document.activeElement).toBe(rowEl(mounted.container, 0));
    return mounted;
  }

  it("End focuses the last row once the host has sent it, Home the first", async () => {
    const { host, tree, container } = await mountFocused(manyRows(1000));

    await fireEvent.keyDown(tree, { key: "End" });
    await tick();

    // The row is outside the window: the tree scrolls there and asks for it.
    expect(tree.scrollTop).toBe((1000 - VIEWPORT_ROWS) * ITEM_HEIGHT);
    expect(host.posted("setViewport")).toEqual([{ start: 929, count: 71 }]);
    expect(focusedRow(container)).toBeNull();

    await answerAll(host);

    expect(focusedRow(container)).toBe(999);
    expect(document.activeElement).toBe(rowEl(container, 999));
    expect(rowEl(container, 999)).toHaveAttribute(
      "data-path",
      "/repo/file-00999.txt",
    );

    await fireEvent.keyDown(rowEl(container, 999), { key: "Home" });
    await tick();
    expect(tree.scrollTop).toBe(0);
    await answerAll(host);

    expect(focusedRow(container)).toBe(0);
    expect(document.activeElement).toBe(rowEl(container, 0));
  });

  it("keeps the focus in the tree while the focused row is not rendered", async () => {
    const { host, tree, container } = await mountFocused(manyRows(1000));

    await fireEvent.keyDown(rowEl(container, 0), { key: "End" });
    await settle();

    // The row that had the focus is gone and the new one has not arrived:
    // the tree itself holds the focus, so the next keys reach its handler.
    expect(focusedRow(container)).toBeNull();
    expect(document.activeElement).toBe(tree);

    await answerAll(host);
    expect(document.activeElement).toBe(rowEl(container, 999));
  });

  it("does not lose keys pressed before the rows have arrived", async () => {
    const { host, tree, container } = await mountFocused(manyRows(1000));

    // End, ArrowUp, Space in quick succession: the host has answered nothing.
    await fireEvent.keyDown(rowEl(container, 0), { key: "End" });
    await tick();
    await fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
    await tick();
    await fireEvent.keyDown(document.activeElement!, { key: " " });
    await settle();
    expect(host.posted("toggleSelection")).toEqual([]);
    expect(document.activeElement).toBe(tree);

    await answerAll(host);

    // ArrowUp moved the focus by position; Space acted on the row it was
    // pressed on, once that row was here.
    expect(focusedRow(container)).toBe(998);
    expect(document.activeElement).toBe(rowEl(container, 998));
    expect(host.posted("toggleSelection")).toEqual(["/repo/file-00998.txt"]);

    // Nothing is left over: a later snapshot does not act again.
    host.answer();
    await settle();
    expect(host.posted("toggleSelection")).toHaveLength(1);
  });

  it("acts on the row a key was pressed on, also when the focus has moved on", async () => {
    const { host, container } = await mountFocused(manyRows(1000));

    await fireEvent.keyDown(rowEl(container, 0), { key: "End" });
    await tick();
    await fireEvent.keyDown(document.activeElement!, { key: "Enter" });
    await fireEvent.keyDown(document.activeElement!, { key: "Home" });
    await answerAll(host);

    expect(host.posted("loadFilePreview")).toEqual(["/repo/file-00999.txt"]);
    expect(focusedRow(container)).toBe(0);
  });

  it("drops keys kept for a row when the list changes before it arrives", async () => {
    const { host, container } = await mountFocused(manyRows(1000));

    await fireEvent.keyDown(rowEl(container, 0), { key: "End" });
    await tick();
    await fireEvent.keyDown(document.activeElement!, { key: " " });
    // Before the rows arrive, the list becomes another one: position 999
    // is not the row the key was pressed on any more.
    host.setRows(manyRows(1200));
    await answerAll(host);

    expect(host.posted("toggleSelection")).toEqual([]);
  });

  it("PageDown and PageUp move by a viewport, also out of the window", async () => {
    const { host, tree, container } = await mountFocused(manyRows(1000));

    for (let page = 1; page <= 12; page++) {
      await fireEvent.keyDown(tree, { key: "PageDown" });
      await answerAll(host);
      expect(focusedRow(container)).toBe(page * VIEWPORT_ROWS);
    }
    expect(document.activeElement).toBe(rowEl(container, 120));
    expect(host.posted("setViewport").length).toBeGreaterThan(0);

    await fireEvent.keyDown(tree, { key: "PageUp" });
    await answerAll(host);
    expect(focusedRow(container)).toBe(110);
  });

  it("ArrowDown walks through the list without the host answering each step late", async () => {
    const { host, tree, container } = await mountFocused(manyRows(1000));

    for (let step = 1; step <= 150; step++) {
      await fireEvent.keyDown(tree, { key: "ArrowDown" });
      // Every row the focus moves to is already there: the window is ahead.
      await tick();
      expect(focusedRow(container)).toBe(step);
      host.answer();
      await tick();
    }
    await settle();

    expect(document.activeElement).toBe(rowEl(container, 150));
    // One request per half window, not one per key.
    const requests = host.posted("setViewport").length;
    expect(requests).toBeGreaterThan(2);
    expect(requests).toBeLessThan(15);
  });

  it("typing finds a row anywhere in the list", async () => {
    const rows = rowsOf([
      ...Array.from({ length: 400 }, (_, i) =>
        file(`/repo/file-${String(i).padStart(5, "0")}.txt`),
      ),
      file("/repo/zebra.txt"),
    ]);
    const { host, tree, container } = await mountFocused(rows);

    await fireEvent.keyDown(tree, { key: "z" });
    await fireEvent.keyDown(tree, { key: "E" });

    // The host looks, from the row after the focused one.
    expect(host.posted("locateRow")).toEqual([
      { request: 1, target: { kind: "typeahead", prefix: "z", from: 1 } },
      { request: 2, target: { kind: "typeahead", prefix: "ze", from: 1 } },
    ]);
    expect(focusedRow(container)).toBe(0);

    await answerAll(host);

    expect(focusedRow(container)).toBe(400);
    expect(document.activeElement).toBe(rowEl(container, 400));
    expect(rowEl(container, 400)).toHaveAttribute(
      "data-path",
      "/repo/zebra.txt",
    );
  });

  it("typing something no row starts with leaves the focus where it is", async () => {
    const { host, tree, container } = await mountFocused(manyRows(300));

    await fireEvent.keyDown(tree, { key: "q" });
    await answerAll(host);

    expect(host.posted("locateRow")).toHaveLength(1);
    expect(focusedRow(container)).toBe(0);
    expect(tree.scrollTop).toBe(0);
  });

  it("ArrowLeft goes to the parent directory, also far above the window", async () => {
    const rows = rowsOf([
      dir(
        "/repo/big",
        true,
        Array.from({ length: 500 }, (_, i) =>
          file(`/repo/big/file-${String(i).padStart(5, "0")}.txt`),
        ),
      ),
    ]);
    const { host, tree, container } = await mountFocused(rows);

    // Inside the window the parent is known: nothing is asked.
    await fireEvent.keyDown(tree, { key: "ArrowDown" });
    await fireEvent.keyDown(tree, { key: "ArrowDown" });
    await tick();
    expect(focusedRow(container)).toBe(2);
    await fireEvent.keyDown(tree, { key: "ArrowLeft" });
    await settle();
    expect(focusedRow(container)).toBe(0);
    expect(host.received).toEqual([]);

    // From row 400 the directory is 400 rows above the window.
    await scrollToRow(tree, 400);
    await answerAll(host);
    rowEl(container, 400).focus();
    await tick();
    expect(focusedRow(container)).toBe(400);
    host.received.length = 0;

    await fireEvent.keyDown(rowEl(container, 400), { key: "ArrowLeft" });

    expect(host.posted("locateRow")).toEqual([
      {
        request: 1,
        target: { kind: "parent_of", path: "/repo/big/file-00399.txt" },
      },
    ]);
    await answerAll(host);

    expect(tree.scrollTop).toBe(0);
    expect(focusedRow(container)).toBe(0);
    expect(document.activeElement).toBe(rowEl(container, 0));
    expect(rowEl(container, 0)).toHaveAttribute("data-path", "/repo/big");
  });

  it("ArrowLeft on a row at the top level does nothing", async () => {
    const { host, tree, container } = await mountFocused(manyRows(300));

    await fireEvent.keyDown(tree, { key: "ArrowDown" });
    await fireEvent.keyDown(tree, { key: "ArrowLeft" });
    await settle();

    expect(host.received).toEqual([]);
    expect(focusedRow(container)).toBe(1);
  });

  it("ArrowRight enters an expanded directory whose first child is the next row", async () => {
    const rows = rowsOf([
      dir("/repo/src", true, [file("/repo/src/a.ts"), file("/repo/src/b.ts")]),
      dir("/repo/empty", true, []),
      file("/repo/z.txt"),
    ]);
    const { host, tree, container } = await mountFocused(rows);

    await fireEvent.keyDown(tree, { key: "ArrowRight" });
    await settle();
    expect(focusedRow(container)).toBe(1);

    // An expanded directory without rows below it: the next row is a sibling.
    rowEl(container, 3).focus();
    await tick();
    await fireEvent.keyDown(rowEl(container, 3), { key: "ArrowRight" });
    await settle();
    expect(focusedRow(container)).toBe(3);
    expect(host.received).toEqual([]);
  });

  it("keys that act on the focused row use the row of the list, not of the window", async () => {
    const { host, tree, container } = await mountFocused(manyRows(1000));
    await scrollToRow(tree, 600);
    await answerAll(host);
    rowEl(container, 605).focus();
    await tick();
    host.received.length = 0;

    await fireEvent.keyDown(rowEl(container, 605), { key: " " });
    await fireEvent.keyDown(rowEl(container, 605), { key: "Enter" });

    expect(host.received).toEqual([
      ["toggleSelection", "/repo/file-00605.txt"],
      ["loadFilePreview", "/repo/file-00605.txt"],
    ]);
  });
});

describe("FileTree – the focused row when the list changes", () => {
  async function mountFocusedOnRow(rows: TreeRow[], row: number) {
    const mounted = await mount(rows);
    await waitFor(() => expect(focusedRow(mounted.container)).toBe(0));
    for (let i = 0; i < row; i++) {
      await fireEvent.keyDown(mounted.tree, { key: "ArrowDown" });
    }
    await settle();
    expect(focusedRow(mounted.container)).toBe(row);
    return mounted;
  }

  const before = (count: number) =>
    rowsOf(
      Array.from({ length: count }, (_, i) =>
        file(`/repo/a-new-${String(i).padStart(4, "0")}.txt`),
      ),
    );

  it("stays on its item when rows appear above it inside the window", async () => {
    const rows = manyRows(1000);
    const { host, container } = await mountFocusedOnRow(rows, 5);

    host.setRows([...before(3), ...rows]);
    host.answer();
    await settle();

    expect(focusedRow(container)).toBe(8);
    expect(rowEl(container, 8)).toHaveAttribute(
      "data-path",
      "/repo/file-00005.txt",
    );
    // The window shows where it is: the host is not asked.
    expect(host.posted("locateRow")).toEqual([]);
  });

  it("asks the host where its item went when it left the window", async () => {
    const rows = manyRows(1000);
    const { host, tree, container } = await mountFocusedOnRow(rows, 5);

    // "Expand all" above it: 300 new rows push it out of the window.
    host.setRows([...before(300), ...rows]);
    host.answer();
    await settle();

    expect(host.posted("locateRow")).toEqual([
      { request: 1, target: { kind: "path", path: "/repo/file-00005.txt" } },
    ]);
    host.answer();
    await settle();

    // The focus is with the item (row 305); the view has not moved.
    expect(tree.scrollTop).toBe(0);
    expect(focusedRow(container)).toBeNull();
    expect(host.posted("locateRow")).toHaveLength(1);

    // The next key continues from there.
    await fireEvent.keyDown(tree, { key: "ArrowDown" });
    await answerAll(host);

    expect(focusedRow(container)).toBe(306);
    expect(rowEl(container, 306)).toHaveAttribute(
      "data-path",
      "/repo/file-00006.txt",
    );
    expect(document.activeElement).toBe(rowEl(container, 306));
  });

  it("keeps the position when its item is no row any more", async () => {
    const rows = manyRows(1000);
    const { host, container } = await mountFocusedOnRow(rows, 5);

    // The focused file is filtered out (or its directory collapsed).
    host.setRows(rows.filter((row) => row.path !== "/repo/file-00005.txt"));
    host.answer();
    await settle();
    expect(host.posted("locateRow")).toHaveLength(1);
    host.answer();
    await settle();

    expect(focusedRow(container)).toBe(5);
    expect(rowEl(container, 5)).toHaveAttribute(
      "data-path",
      "/repo/file-00006.txt",
    );
    // That row is the focused one from now on: no further question.
    host.answer();
    await settle();
    expect(host.posted("locateRow")).toHaveLength(1);
  });

  it("gives the focus back to its row when that is scrolled into view again", async () => {
    const { host, tree, container } = await mountFocusedOnRow(
      manyRows(1000),
      5,
    );
    expect(document.activeElement).toBe(rowEl(container, 5));

    await scrollToRow(tree, 700);
    await answerAll(host);
    // Its element is gone; the focus waits on the tree, not on the body.
    expect(document.activeElement).toBe(tree);

    await scrollToRow(tree, 0);
    await answerAll(host);
    expect(document.activeElement).toBe(rowEl(container, 5));
  });

  it("does not ask while the list is the same and the item is just out of sight", async () => {
    const { host, tree, container } = await mountFocusedOnRow(
      manyRows(1000),
      5,
    );

    await scrollToRow(tree, 700);
    await answerAll(host);
    expect(focusedRow(container)).toBeNull();
    await scrollToRow(tree, 0);
    await answerAll(host);

    expect(host.posted("locateRow")).toEqual([]);
    expect(focusedRow(container)).toBe(5);
  });

  it("ignores an answer it has not asked for", async () => {
    const rows = manyRows(1000);
    const { host, tree, container } = await mountFocusedOnRow(rows, 5);

    // A lookup of an earlier session of the UI is still in the host's state.
    host.receive("locateRow", {
      request: 1,
      target: { kind: "path", path: "/repo/file-00500.txt" },
    });
    host.received.length = 0;
    host.answer();
    await settle();

    expect(focusedRow(container)).toBe(5);
    expect(tree.scrollTop).toBe(0);
    expect(host.received).toEqual([]);
  });
});
