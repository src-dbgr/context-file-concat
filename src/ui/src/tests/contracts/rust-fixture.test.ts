/* @vitest-environment node */

// Cross-language contract test.
//
// `fixtures/ui_state_pruned.json` is written by the Rust backend
// (src/app/view_model/contract_tests.rs) and compared there against what
// `generate_ui_state` really serializes. Here the same file must pass the full
// runtime schema, so Rust and TypeScript cannot drift apart without CI failing
// on one of the two sides.
//
// Regenerate after an intended change: `UPDATE_CONTRACT_FIXTURES=1 cargo test contract_fixture`

import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { UiStateSchema, UiStateEnvelopeSchema } from "$lib/ipc/schema";
import type { AppState, TreeNode } from "$lib/types";

const fixturePath = fileURLToPath(
  new URL("./fixtures/ui_state_pruned.json", import.meta.url),
);
const fixture: unknown = JSON.parse(readFileSync(fixturePath, "utf8"));

/** Every key of a JSON object tree, as dotted paths with array indices dropped. */
function keyPaths(value: unknown, prefix = "", out = new Set<string>()) {
  if (Array.isArray(value)) {
    for (const entry of value) keyPaths(entry, prefix, out);
  } else if (typeof value === "object" && value !== null) {
    for (const [key, child] of Object.entries(value)) {
      const path = prefix ? `${prefix}.${key}` : key;
      out.add(path);
      keyPaths(child, path, out);
    }
  }
  return out;
}

function allNodes(nodes: TreeNode[]): TreeNode[] {
  return nodes.flatMap((n) => [n, ...allNodes(n.children)]);
}

function find(nodes: TreeNode[], path: string): TreeNode {
  const hit = allNodes(nodes).find((n) => n.path === path);
  if (!hit) throw new Error(`${path} is not part of the fixture`);
  return hit;
}

describe("[IPC Contracts] UiState serialized by the Rust backend", () => {
  it("passes the full UiStateSchema", () => {
    const result = UiStateSchema.safeParse(fixture);
    expect(result.error?.issues ?? []).toEqual([]);
    expect(result.success).toBe(true);
  });

  it("passes the production envelope schema", () => {
    expect(UiStateEnvelopeSchema.safeParse(fixture).success).toBe(true);
  });

  it("contains no top-level field the schema does not know", () => {
    // UiStateSchema is passthrough, so an unknown backend field would not
    // fail the parse. Compare the key sets explicitly instead.
    const known = Object.keys(UiStateSchema.shape).sort();
    const sent = Object.keys(fixture as Record<string, unknown>).sort();
    expect(sent).toEqual(known);
  });

  it("is parsed without losing or inventing fields", () => {
    const parsed = UiStateSchema.parse(fixture);
    expect([...keyPaths(parsed)].sort()).toEqual([...keyPaths(fixture)].sort());
  });

  it("sends the pruning flags on every node", () => {
    const { tree } = UiStateSchema.parse(fixture);
    const nodes = allNodes(tree);
    expect(nodes.length).toBeGreaterThan(0);
    for (const node of nodes) {
      expect(typeof node.has_children, node.path).toBe("boolean");
      expect(typeof node.children_loaded, node.path).toBe("boolean");
    }
  });

  it("sends children only for expanded directories", () => {
    const { tree } = UiStateSchema.parse(fixture);
    for (const node of allNodes(tree)) {
      if (!(node.is_directory && node.is_expanded)) {
        expect(node.children, node.path).toEqual([]);
      }
    }
  });

  it("describes collapsed directories completely", () => {
    const state = UiStateSchema.parse(fixture) as unknown as AppState;

    expect(find(state.tree, "/project/src")).toMatchObject({
      is_expanded: true,
      has_children: true,
      selection_state: "partial",
    });
    expect(
      find(state.tree, "/project/src").children.map((n) => n.name),
    ).toEqual(["core", "lib.rs", "main.rs"]);
    // Collapsed inside an expanded directory: all hidden files are selected.
    expect(find(state.tree, "/project/src/core")).toMatchObject({
      is_expanded: false,
      children: [],
      has_children: true,
      children_loaded: true,
      selection_state: "full",
    });
    expect(find(state.tree, "/project/docs")).toMatchObject({
      children: [],
      has_children: true,
      selection_state: "none",
    });
    expect(find(state.tree, "/project/vendor")).toMatchObject({
      has_children: false,
      children_loaded: false,
    });
    expect(find(state.tree, "/project/empty")).toMatchObject({
      has_children: false,
      children_loaded: true,
    });
    expect(find(state.tree, "/project/src/main.rs")).toMatchObject({
      is_match: true,
      is_previewed: true,
      selection_state: "full",
    });
  });

  it("carries the totals of the complete tree, not of the pruned one", () => {
    const state = UiStateSchema.parse(fixture);
    const sent = allNodes(state.tree);

    expect(state.tree_files_count).toBe(7);
    expect(state.tree_dirs_count).toBe(5);
    expect(sent.filter((n) => !n.is_directory).length).toBeLessThan(
      state.tree_files_count,
    );
    expect(state.selected_files_count).toBe(3);
  });
});
