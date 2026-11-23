import { describe, it, expect } from "vitest";
import fc from "fast-check";
import {
  ITEM_HEIGHT,
  MAX_WINDOW_ROWS,
  MIN_BUFFER_ROWS,
  OVERSCAN,
  renderRange,
  visibleRange,
  windowToRequest,
  type RowWindow,
} from "$lib/modules/treeWindow";

const px = (rows: number) => rows * ITEM_HEIGHT;

describe("treeWindow – visibleRange", () => {
  it("lists the rows that are at least partly inside the viewport", () => {
    expect(visibleRange(0, px(10), 1000)).toEqual({ start: 0, end: 10 });
    expect(visibleRange(px(5), px(10), 1000)).toEqual({ start: 5, end: 15 });
    // Half a row scrolled: the rows cut at the top and at the bottom count.
    expect(visibleRange(px(5) + 14, px(10), 1000)).toEqual({
      start: 5,
      end: 16,
    });
  });

  it("stays inside the list", () => {
    expect(visibleRange(px(995), px(10), 1000)).toEqual({
      start: 995,
      end: 1000,
    });
    // Scrolled further than the list is long (it has just become shorter).
    expect(visibleRange(px(5000), px(10), 1000)).toEqual({
      start: 999,
      end: 1000,
    });
    // Overscrolled above the top (rubber band): no row before the first.
    expect(visibleRange(-ITEM_HEIGHT, px(10), 1000)).toEqual({
      start: 0,
      end: 9,
    });
    expect(visibleRange(0, px(10), 3)).toEqual({ start: 0, end: 3 });
  });

  it("is one row for a viewport that has not been measured, none without rows", () => {
    expect(visibleRange(0, 0, 1000)).toEqual({ start: 0, end: 1 });
    expect(visibleRange(px(7), -1, 1000)).toEqual({ start: 7, end: 8 });
    expect(visibleRange(0, px(10), 0)).toEqual({ start: 0, end: 0 });
  });
});

describe("treeWindow – renderRange", () => {
  it("adds the overscan on both sides", () => {
    expect(renderRange(px(100), px(10), 1000)).toEqual({
      start: 100 - OVERSCAN,
      end: 110 + OVERSCAN,
    });
    expect(renderRange(0, px(10), 1000)).toEqual({
      start: 0,
      end: 10 + OVERSCAN,
    });
    expect(renderRange(px(990), px(10), 1000)).toEqual({
      start: 990 - OVERSCAN,
      end: 1000,
    });
  });

  it("renders the whole list if the viewport is at least as tall", () => {
    expect(renderRange(0, px(10), 10)).toEqual({ start: 0, end: 10 });
    expect(renderRange(0, px(40), 10)).toEqual({ start: 0, end: 10 });
    expect(renderRange(0, px(10), 0)).toEqual({ start: 0, end: 0 });
  });
});

describe("treeWindow – windowToRequest", () => {
  /** A viewport of `rows` rows whose first visible row is `first`. */
  const request = (first: number, held: RowWindow, total = 10_000, rows = 10) =>
    windowToRequest(px(first), px(rows), held, total);

  // A viewport of 10 rows can show 11 (one cut at either end); it is asked for
  // with MIN_BUFFER_ROWS rows on either side.
  const SMALL = 11 + 2 * MIN_BUFFER_ROWS;

  it("asks for nothing while the window holds a margin around the viewport", () => {
    const held: RowWindow = { start: 0, count: 100 };
    expect(request(0, held)).toBeNull();
    expect(request(40, held)).toBeNull();
    // The margin is half the buffer.
    const lastQuiet = 100 - 10 - MIN_BUFFER_ROWS / 2;
    expect(request(lastQuiet, held)).toBeNull();
    expect(request(lastQuiet + 1, held)).not.toBeNull();
  });

  it("asks for the rows of the viewport with a buffer on either side", () => {
    const held: RowWindow = { start: 0, count: 100 };
    expect(request(500, held)).toEqual({
      start: 500 - MIN_BUFFER_ROWS,
      count: SMALL,
    });
    // A tall viewport: one viewport of rows as buffer.
    expect(request(500, held, 10_000, 50)).toEqual({
      start: 500 - 51,
      count: 3 * 51,
    });
  });

  it("asks for the same window wherever the viewport is inside a row", () => {
    const held: RowWindow = { start: 0, count: 100 };
    const aligned = windowToRequest(px(500), px(10), held, 10_000);
    for (const offset of [1, 13, ITEM_HEIGHT - 1]) {
      expect(windowToRequest(px(500) + offset, px(10), held, 10_000)).toEqual(
        aligned,
      );
    }
  });

  it("keeps the window inside the list", () => {
    const held: RowWindow = { start: 0, count: 100 };
    expect(request(9_990, held)).toEqual({
      start: 10_000 - SMALL,
      count: SMALL,
    });
    expect(request(0, { start: 5_000, count: SMALL })).toEqual({
      start: 0,
      count: SMALL,
    });
    // A list shorter than the window is asked for as a whole.
    expect(request(0, { start: 0, count: 5 }, 40)).toEqual({
      start: 0,
      count: 40,
    });
  });

  it("asks for nothing without rows, or when everything is held", () => {
    expect(windowToRequest(0, px(10), { start: 0, count: 0 }, 0)).toBeNull();
    expect(request(0, { start: 0, count: 40 }, 40)).toBeNull();
  });

  it("never asks for more than the host sends", () => {
    const held: RowWindow = { start: 0, count: 100 };
    expect(request(5_000, held, 100_000, 400)).toEqual({
      start: 5_000 - Math.floor((MAX_WINDOW_ROWS - 401) / 2),
      count: MAX_WINDOW_ROWS,
    });
  });

  it("does not ask again for the window it holds", () => {
    // A viewport nearly as large as the largest window: what the host sends
    // never covers the margin, but asking again would not change that.
    const first = request(5_000, { start: 0, count: 100 }, 100_000, 590);
    expect(first).not.toBeNull();
    expect(request(5_000, first!, 100_000, 590)).toBeNull();
  });

  it("property: the answer of the host to a request needs no further request", () => {
    fc.assert(
      fc.property(
        fc.integer({ min: 1, max: 200_000 }),
        fc.integer({ min: 0, max: 6_000_000 }),
        fc.integer({ min: 0, max: 3_000 }),
        fc.integer({ min: 0, max: 200_000 }),
        fc.integer({ min: 0, max: MAX_WINDOW_ROWS }),
        (total, scrollTop, viewportHeight, heldStart, heldCount) => {
          const visible = visibleRange(scrollTop, viewportHeight, total);
          const count = Math.min(heldCount, total);
          const held = { start: Math.min(heldStart, total - count), count };
          const asked = windowToRequest(scrollTop, viewportHeight, held, total);
          if (asked === null) return;

          // The request is a window of the list...
          expect(asked.count).toBeGreaterThan(0);
          expect(asked.count).toBeLessThanOrEqual(MAX_WINDOW_ROWS);
          expect(asked.start).toBeGreaterThanOrEqual(0);
          expect(asked.start + asked.count).toBeLessThanOrEqual(total);
          // ...that holds every visible row...
          expect(asked.start).toBeLessThanOrEqual(visible.start);
          expect(asked.start + asked.count).toBeGreaterThanOrEqual(visible.end);
          // ...and once the host has sent it, the UI is content.
          expect(
            windowToRequest(scrollTop, viewportHeight, asked, total),
          ).toBeNull();
        },
      ),
    );
  });
});
