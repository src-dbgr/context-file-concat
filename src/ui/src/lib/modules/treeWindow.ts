// Which rows of the tree the UI shows, renders and asks the host for.
//
// The host owns the list of all rows (every item whose ancestors are
// expanded). The UI knows how long that list is (`rows_total`, which sizes
// the scrollbar) and holds a window of it (`rows` from `rows_start` on). When
// the viewport moves towards the edge of that window, the UI reports where it
// is (`setViewport`) and the next snapshot carries the rows around it.
//
// This module is the arithmetic of that, without state and without the DOM.

/** The height of a row in px. Fixed: rows are positioned by their index. */
export const ITEM_HEIGHT = 28;
/** Rows rendered above and below the viewport. */
export const OVERSCAN = 5;
/**
 * The least number of rows held above and below the viewport, for a viewport
 * that is small or has not been measured yet.
 */
export const MIN_BUFFER_ROWS = 30;
/**
 * The largest window the host sends (`Viewport::MAX_ROWS` in
 * `src/app/view_model/rows.rs`). Asking for more would be answered with less.
 * The host states its limits in the scenario fixture of the contract tests,
 * and `tree-window-scenario.test.ts` compares these constants with them.
 */
export const MAX_WINDOW_ROWS = 600;
/**
 * The window the host sends before the UI has reported its viewport
 * (`Viewport::DEFAULT_ROWS`).
 */
export const DEFAULT_WINDOW_ROWS = 100;

/** Rows `start` (inclusive) to `end` (exclusive) of the list of all rows. */
export type RowRange = { start: number; end: number };
/** `count` rows from row `start` on: the shape of `setViewport`. */
export type RowWindow = { start: number; count: number };

const clamp = (value: number, min: number, max: number) =>
  Math.min(Math.max(value, min), max);

/** The rows that are at least partly inside the viewport. */
export function visibleRange(
  scrollTop: number,
  viewportHeight: number,
  total: number,
): RowRange {
  if (total <= 0) return { start: 0, end: 0 };
  const start = clamp(Math.floor(scrollTop / ITEM_HEIGHT), 0, total - 1);
  const end = clamp(
    Math.ceil((scrollTop + Math.max(0, viewportHeight)) / ITEM_HEIGHT),
    start + 1,
    total,
  );
  return { start, end };
}

/**
 * The rows to render: the visible ones plus the overscan. A viewport that is
 * at least as tall as the whole list shows all of it.
 */
export function renderRange(
  scrollTop: number,
  viewportHeight: number,
  total: number,
): RowRange {
  if (total <= 0) return { start: 0, end: 0 };
  if (Math.max(0, viewportHeight) >= total * ITEM_HEIGHT) {
    return { start: 0, end: total };
  }
  const visible = visibleRange(scrollTop, viewportHeight, total);
  return {
    start: Math.max(0, visible.start - OVERSCAN),
    end: Math.min(total, visible.end + OVERSCAN),
  };
}

function covers(held: RowWindow, range: RowRange): boolean {
  return held.start <= range.start && range.end <= held.start + held.count;
}

/**
 * The window to ask the host for, or `null` while the window the UI holds is
 * enough.
 *
 * The UI asks for the rows of the viewport plus one viewport of rows (at
 * least {@link MIN_BUFFER_ROWS}) on either side, and asks again when less than
 * half of that buffer is left in the direction of travel. So scrolling by a
 * row sends nothing, a page of keyboard navigation stays inside the window,
 * and a jump is answered with one snapshot. The request depends on the first
 * visible row and the height of the viewport only: scrolling by a few pixels
 * while an answer is under way asks for the same window again, which the
 * caller can tell.
 *
 * `held` is the window of the snapshot at hand. A window that is exactly what
 * the host makes of the request is never asked for again, so a window that
 * cannot be had any larger (see {@link MAX_WINDOW_ROWS}) is not asked for in
 * a loop.
 */
export function windowToRequest(
  scrollTop: number,
  viewportHeight: number,
  held: RowWindow,
  total: number,
): RowWindow | null {
  if (total <= 0) return null;

  const visible = visibleRange(scrollTop, viewportHeight, total);
  // The rows a viewport of this height can show, a cut one at either end.
  const viewportRows = Math.ceil(Math.max(0, viewportHeight) / ITEM_HEIGHT) + 1;
  const buffer = Math.max(viewportRows, MIN_BUFFER_ROWS);
  const margin = Math.floor(buffer / 2);
  const needed: RowRange = {
    start: Math.max(0, visible.start - margin),
    end: Math.min(total, visible.end + margin),
  };
  if (covers(held, needed)) return null;

  const count = Math.min(viewportRows + 2 * buffer, MAX_WINDOW_ROWS, total);
  const before = Math.max(0, Math.floor((count - viewportRows) / 2));
  const start = clamp(visible.start - before, 0, total - count);
  if (held.start === start && held.count === count) return null;
  return { start, count };
}
