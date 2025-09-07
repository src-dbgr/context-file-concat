/**
 * Bundle budget checker (Node 20+/24+, ESM).
 * - No external deps.
 * - Computes Brotli sizes for built assets in src/ui/dist/assets.
 *
 * Categories:
 *   - ENTRY  : main entry bundle (index.js or parsed from dist/index.html)
 *   - APP    : all non-worker JS/CSS assets (application code)
 *   - WORKERS: *.worker-*.js (Monaco editor/ts/css/html/json workers, etc.)
 *
 * Env overrides (bytes):
 *   ENTRY_BROTLI          default: 800_000 (~781.3 KB)
 *   APP_TOTAL_BROTLI      default: 1_000_000 (1.0 MB)
 *   WORKER_TOTAL_BROTLI   default: 1_500_000 (1.5 MB)
 *
 * Back-compat:
 *   - If BUDGET_TOTAL_BROTLI is set (legacy), enforce GRAND TOTAL <= that.
 */

import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";

const UI_DIR = path.resolve(process.cwd(), ".");
const DIST_DIR = path.resolve(UI_DIR, "dist");
const DIST_ASSETS = path.resolve(DIST_DIR, "assets");

const ENTRY_LIMIT = num(process.env.ENTRY_BROTLI, 800_000);
const APP_LIMIT = num(process.env.APP_TOTAL_BROTLI, 1_000_000);
const WORKER_LIMIT = num(process.env.WORKER_TOTAL_BROTLI, 1_500_000);
const LEGACY_TOTAL = process.env.BUDGET_TOTAL_BROTLI
  ? num(process.env.BUDGET_TOTAL_BROTLI)
  : null;

function num(v, d) {
  const n = Number(v);
  return Number.isFinite(n) ? n : d;
}

function brotliSize(buf) {
  const out = zlib.brotliCompressSync(buf, {
    params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11 },
  });
  return out.byteLength;
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

function listAssets(dir) {
  if (!fs.existsSync(dir)) {
    throw new Error(
      `Assets directory not found: ${dir}. Did you run "npm run build"?`
    );
  }
  return fs
    .readdirSync(dir)
    .filter((f) => /\.(js|css)$/.test(f))
    .map((f) => path.join(dir, f));
}

function read(p) {
  return fs.readFileSync(p);
}

function findEntryAsset(files) {
  // Prefer "index.js" given our rollup output name
  const byIndex = files.find((f) => path.basename(f) === "index.js");
  if (byIndex) return byIndex;

  // Also accept "main.js" if config changes someday
  const byMain = files.find((f) => path.basename(f) === "main.js");
  if (byMain) return byMain;

  // Parse built index.html for <script src="/assets/xxx.js">
  const indexHtml = path.join(DIST_DIR, "index.html");
  if (fs.existsSync(indexHtml)) {
    const html = fs.readFileSync(indexHtml, "utf8");
    const m = html.match(/src="\/assets\/([^"]+\.js)"/);
    if (m) {
      const cand = path.join(DIST_ASSETS, m[1]);
      if (fs.existsSync(cand)) return cand;
    }
  }

  // Heuristic fallback: largest JS file
  const js = files.filter((f) => f.endsWith(".js"));
  if (js.length === 0) return null;
  return (
    js.sort((a, b) => fs.statSync(b).size - fs.statSync(a).size)[0] ?? null
  );
}

function isWorker(filePath) {
  const b = path.basename(filePath);
  // Match *.worker-<hash>.js (Vite) and common Monaco worker names
  return /\.worker[^/]*\.js$/.test(b);
}

function table(title, rows) {
  const pad = (s, n) => String(s).padEnd(n, " ");
  const widths = [30, 12, 12];
  console.log(`\n${title}`);
  console.log(
    pad("Asset", widths[0]),
    pad("Raw", widths[1]),
    pad("Brotli", widths[2])
  );
  console.log("-".repeat(widths.reduce((a, b) => a + b, 0)));
  for (const r of rows) {
    console.log(
      pad(r.name, widths[0]),
      pad(prettyBytes(r.raw), widths[1]),
      pad(prettyBytes(r.br), widths[2])
    );
  }
}

function main() {
  const files = listAssets(DIST_ASSETS);
  const rows = files.map((f) => {
    const buf = read(f);
    return {
      name: path.basename(f),
      raw: buf.byteLength,
      br: brotliSize(buf),
      path: f,
      isWorker: isWorker(f),
      isJS: f.endsWith(".js"),
      isCSS: f.endsWith(".css"),
    };
  });

  // Categorize
  const entryPath = findEntryAsset(rows.map((r) => r.path));
  const entry = entryPath
    ? (rows.find((r) => r.path === entryPath) ?? null)
    : null;
  const workerRows = rows.filter((r) => r.isWorker);
  const appRows = rows.filter((r) => !r.isWorker);

  // Print compact tables
  table("All assets", rows);

  const appTotal = appRows.reduce((a, r) => a + r.br, 0);
  const workerTotal = workerRows.reduce((a, r) => a + r.br, 0);
  const grandTotal = appTotal + workerTotal;

  console.log("");
  console.log(
    `Entry file: ${entry ? entry.name : "(not found)"} (limit ${prettyBytes(
      ENTRY_LIMIT
    )})`
  );
  console.log(
    `App total Brotli (no workers): ${prettyBytes(appTotal)} (limit ${prettyBytes(APP_LIMIT)})`
  );
  console.log(
    `Worker total Brotli:          ${prettyBytes(workerTotal)} (limit ${prettyBytes(WORKER_LIMIT)})`
  );
  if (LEGACY_TOTAL != null) {
    console.log(
      `Grand total Brotli:           ${prettyBytes(grandTotal)} (legacy limit ${prettyBytes(LEGACY_TOTAL)})`
    );
  }
  console.log("");

  const violations = [];
  if (entry && entry.br > ENTRY_LIMIT) {
    violations.push(
      `Entry ${entry.name} exceeds ${prettyBytes(ENTRY_LIMIT)} (got ${prettyBytes(entry.br)})`
    );
  }
  if (appTotal > APP_LIMIT) {
    violations.push(
      `APP total exceeds ${prettyBytes(APP_LIMIT)} (got ${prettyBytes(appTotal)})`
    );
  }
  if (workerTotal > WORKER_LIMIT) {
    violations.push(
      `WORKER total exceeds ${prettyBytes(WORKER_LIMIT)} (got ${prettyBytes(workerTotal)})`
    );
  }
  if (LEGACY_TOTAL != null && grandTotal > LEGACY_TOTAL) {
    violations.push(
      `GRAND total exceeds ${prettyBytes(LEGACY_TOTAL)} (got ${prettyBytes(grandTotal)})`
    );
  }

  if (violations.length) {
    console.error("✖ Budget violations:");
    for (const v of violations) console.error(" -", v);
    process.exit(1);
  } else {
    console.log("✔ Bundle budgets OK");
  }
}

main();
