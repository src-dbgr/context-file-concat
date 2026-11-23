/**
 * Bundle budget checker (Node 24+, ESM, no external deps).
 * Computes Brotli sizes of the built JS/CSS in dist/assets and enforces three budgets.
 *
 * Categories (every JS/CSS asset is in exactly one):
 *   - INITIAL: what the page loads before it can start – everything dist/index.html
 *              references (entry script, modulepreload links, stylesheets) plus the
 *              transitive static imports of those scripts.
 *   - WORKERS: *.worker*.js (Monaco editor/ts/css/html/json workers).
 *   - LAZY   : everything else, i.e. chunks that are only fetched on demand.
 *
 * Env overrides (bytes, Brotli):
 *   INITIAL_BROTLI        default:   150_000
 *   LAZY_TOTAL_BROTLI     default: 1_000_000
 *   WORKER_TOTAL_BROTLI   default: 1_500_000
 */

import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";

const DIST_DIR = path.resolve(process.cwd(), "dist");
const DIST_ASSETS = path.join(DIST_DIR, "assets");
const INDEX_HTML = path.join(DIST_DIR, "index.html");

const INITIAL_LIMIT = num(process.env.INITIAL_BROTLI, 150_000);
const LAZY_LIMIT = num(process.env.LAZY_TOTAL_BROTLI, 1_000_000);
const WORKER_LIMIT = num(process.env.WORKER_TOTAL_BROTLI, 1_500_000);

function num(v, d) {
  const n = Number(v);
  return v !== undefined && v !== "" && Number.isFinite(n) ? n : d;
}

function brotliSize(buf) {
  return zlib.brotliCompressSync(buf, {
    params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11 },
  }).byteLength;
}

function prettyBytes(n) {
  const units = ["B", "KB", "MB"];
  let u = 0;
  let v = n;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v % 1 === 0 ? v : v.toFixed(1)} ${units[u]}`;
}

function isWorker(name) {
  return /\.worker[^/]*\.js$/.test(name);
}

/** Local JS/CSS files that `index.html` makes the browser fetch right away. */
function htmlReferences(html) {
  const refs = [];
  for (const [tag] of html.matchAll(/<(?:script|link)\b[^>]*>/g)) {
    const isScript = tag.startsWith("<script");
    const rel = tag.match(/\brel="([^"]*)"/)?.[1] ?? "";
    if (!isScript && !/^(modulepreload|preload|stylesheet)$/.test(rel)) {
      continue;
    }
    const url = tag.match(/\b(?:src|href)="([^"]+)"/)?.[1];
    if (url && /\.(js|css)$/.test(url) && !/^([a-z]+:)?\/\//.test(url)) {
      refs.push(path.resolve(DIST_DIR, url.replace(/^\//, "")));
    }
  }
  return refs;
}

/**
 * Static imports of a built ES module (`import … from "./x.js"`, `import "./x.js"`,
 * `export … from "./x.js"`). Dynamic `import("./x.js")` is deliberately not matched:
 * those chunks are lazy.
 */
function staticImports(file) {
  const code = fs.readFileSync(file, "utf8");
  const out = [];
  for (const m of code.matchAll(
    /(?:\bfrom|\bimport)\s*["'](\.{1,2}\/[^"']+\.js)["']/g,
  )) {
    out.push(path.resolve(path.dirname(file), m[1]));
  }
  return out;
}

/** Everything the initial page load fetches: HTML references + static import closure. */
function initialFiles() {
  if (!fs.existsSync(INDEX_HTML)) {
    throw new Error(`${INDEX_HTML} not found. Did you run "npm run build"?`);
  }
  const seen = new Set();
  const queue = htmlReferences(fs.readFileSync(INDEX_HTML, "utf8"));
  if (!queue.some((f) => f.endsWith(".js"))) {
    throw new Error(`No entry script found in ${INDEX_HTML}`);
  }
  while (queue.length) {
    const file = queue.pop();
    if (seen.has(file)) continue;
    if (!fs.existsSync(file)) {
      throw new Error(`Initial-load asset is missing from the build: ${file}`);
    }
    seen.add(file);
    if (file.endsWith(".js")) queue.push(...staticImports(file));
  }
  return seen;
}

function table(title, rows) {
  const pad = (s, n) => String(s).padEnd(n, " ");
  console.log(`\n${title}`);
  console.log(pad("Asset", 30), pad("Raw", 12), pad("Brotli", 12));
  console.log("-".repeat(54));
  for (const r of rows) {
    console.log(
      pad(r.name, 30),
      pad(prettyBytes(r.raw), 12),
      pad(prettyBytes(r.br), 12),
    );
  }
}

function main() {
  const initial = initialFiles();
  const rows = fs
    .readdirSync(DIST_ASSETS)
    .filter((f) => /\.(js|css)$/.test(f))
    .map((name) => {
      const file = path.join(DIST_ASSETS, name);
      const buf = fs.readFileSync(file);
      const category = initial.has(file)
        ? "initial"
        : isWorker(name)
          ? "worker"
          : "lazy";
      return { name, raw: buf.byteLength, br: brotliSize(buf), category };
    });

  const budgets = [
    { category: "initial", label: "Initial load", limit: INITIAL_LIMIT },
    { category: "lazy", label: "Lazy chunks", limit: LAZY_LIMIT },
    { category: "worker", label: "Workers", limit: WORKER_LIMIT },
  ];

  const violations = [];
  for (const b of budgets) {
    const own = rows
      .filter((r) => r.category === b.category)
      .sort((x, y) => y.br - x.br);
    const total = own.reduce((a, r) => a + r.br, 0);
    // The initial load is listed in full; of the many lazy chunks only the largest.
    const shown = b.category === "initial" ? own : own.slice(0, 8);
    table(
      `${b.label}: ${prettyBytes(total)} Brotli in ${own.length} file(s), ` +
        `limit ${prettyBytes(b.limit)} (${total} / ${b.limit} bytes)` +
        (shown.length < own.length ? ` – largest ${shown.length} shown` : ""),
      shown,
    );
    if (total > b.limit) {
      violations.push(
        `${b.label} exceeds ${prettyBytes(b.limit)} (got ${prettyBytes(total)})`,
      );
    }
  }

  console.log("");
  if (violations.length) {
    console.error("✖ Budget violations:");
    for (const v of violations) console.error(" -", v);
    process.exit(1);
  }
  console.log("✔ Bundle budgets OK");
}

main();
