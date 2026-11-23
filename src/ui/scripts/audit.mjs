/**
 * `npm audit` gate with an explicit ignore list (Node 24+, ESM, no external deps).
 *
 * Fails on every advisory of severity high or critical that is not listed in IGNORED,
 * and on every IGNORED entry that is no longer reported (so the list cannot go stale).
 * `npm audit` itself has no way to accept a single advisory.
 */

import { spawnSync } from "node:child_process";

/** GHSA id -> why it is acceptable. State when the entry goes away. */
const IGNORED = {
  // tinypool <= 2.1.1 (via vitest 3): prototype-pollution *gadgets* – they need an
  // already polluted Object.prototype inside the test runner process. tinypool is a
  // dev-only dependency of the unit test runner and is not part of the shipped bundle.
  // The fix requires vitest >= 5 (two majors); remove both entries with that upgrade.
  "GHSA-5gmw-xhrv-c9v3": "tinypool via vitest 3, dev-only test runner",
  "GHSA-85c8-ppgw-ccpr": "tinypool via vitest 3, dev-only test runner",
};

const FAILING = new Set(["high", "critical"]);

const res = spawnSync("npm audit --json", {
  encoding: "utf8",
  shell: true,
  maxBuffer: 64 * 1024 * 1024,
});

let report;
try {
  report = JSON.parse(res.stdout);
} catch {
  console.error("npm audit did not return JSON:\n", res.stdout, res.stderr);
  process.exit(2);
}
if (report.error || !report.vulnerabilities) {
  console.error("npm audit failed:", JSON.stringify(report.error ?? report));
  process.exit(2);
}

/** id -> advisory, for every advisory npm reports (transitive entries are strings). */
const advisories = new Map();
for (const vuln of Object.values(report.vulnerabilities)) {
  for (const via of vuln.via) {
    if (typeof via === "object") {
      advisories.set(via.url.split("/").pop(), via);
    }
  }
}

const open = [];
for (const [id, a] of advisories) {
  if (!FAILING.has(a.severity)) continue;
  if (id in IGNORED) {
    console.log(`ignored  ${id} ${a.name} (${a.severity}): ${IGNORED[id]}`);
  } else {
    open.push(`${id} ${a.name} (${a.severity}): ${a.title}`);
  }
}
const stale = Object.keys(IGNORED).filter((id) => !advisories.has(id));

for (const line of open) console.error(`✖ ${line}`);
for (const id of stale) {
  console.error(`✖ ${id} is ignored but no longer reported – remove the entry`);
}
if (open.length || stale.length) process.exit(1);
console.log("✔ No unaccepted npm advisories of severity high or critical");
