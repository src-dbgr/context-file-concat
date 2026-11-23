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
import {
  AnyCommandMessageSchema,
  CommandSchemas,
  ConfigOptionSchema,
  OutputSizeNoticeSchema,
  SaveFilePayloadSchema,
  SaveResultSchema,
  ShowGeneratedArgsSchema,
  UiConfigSchema,
  UiStateSchema,
  UiStateEnvelopeSchema,
  type CommandName,
} from "$lib/ipc/schema";
import type { z } from "zod";
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

  it("sends exactly the config fields the schema knows", () => {
    // UiConfigSchema drops unknown keys, so compare the key sets explicitly:
    // the host must not send config the UI has no use for (it could come
    // back outdated), and the UI must not expect what the host does not send.
    const known = Object.keys(UiConfigSchema.shape).sort();
    const sent = Object.keys(
      (fixture as { config: Record<string, unknown> }).config,
    ).sort();
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

// The other direction: `fixtures/commands.json` is written by the host from its
// routing table (src/app/command_contract_tests.rs): every command name with
// example payloads, serialized from the payload types of the handlers.
const commandFixture = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("./fixtures/commands.json", import.meta.url)),
    "utf8",
  ),
) as Record<string, unknown[]>;

describe("[IPC Contracts] commands routed by the Rust backend", () => {
  it("knows exactly the commands of the host", () => {
    expect(Object.keys(CommandSchemas).sort()).toEqual(
      Object.keys(commandFixture).sort(),
    );
  });

  it("has at least one example payload per command", () => {
    for (const [name, payloads] of Object.entries(commandFixture)) {
      expect(Array.isArray(payloads) && payloads.length > 0, name).toBe(true);
    }
  });

  it("accepts every example payload with the schema of its command", () => {
    for (const [name, payloads] of Object.entries(commandFixture)) {
      const schema = CommandSchemas[name as CommandName];
      for (const payload of payloads) {
        const label = `${name} ${JSON.stringify(payload)}`;
        const result = schema.safeParse(payload);
        expect(result.error?.issues ?? [], label).toEqual([]);
        // Nothing is dropped or added on the way through the schema, so
        // what `post()` sends in DEV is what the host has serialized.
        expect(result.data, label).toEqual(payload);
        expect(
          AnyCommandMessageSchema.safeParse({ command: name, payload }).success,
          label,
        ).toBe(true);
      }
    }
  });

  it("covers every option of setConfigOption on both sides", () => {
    const hostOptions = new Set(
      commandFixture.setConfigOption.map(
        (payload) => (payload as { option: string }).option,
      ),
    );
    const uiOptions = new Set(
      ConfigOptionSchema.options.map((o) => o.shape.option.value),
    );
    expect(hostOptions).toEqual(uiOptions);
  });
});

describe("[IPC Contracts] saveFile as the host accepts it", () => {
  it("covers every kind of save on both sides", () => {
    const hostKinds = new Set(
      commandFixture.saveFile.map(
        (payload) => (payload as { kind: string }).kind,
      ),
    );
    const uiKinds = new Set(
      SaveFilePayloadSchema.options.map((o) => o.shape.kind.value),
    );
    expect(hostKinds).toEqual(uiKinds);
  });
});

// Host → UI: `fixtures/output_events.json` is written by the host
// (src/app/event_contract_tests.rs) from the scripts it really evaluates in
// the WebView: for each handler of the generated output, the argument lists of
// example calls, one per form the event can take.
const outputEvents = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("./fixtures/output_events.json", import.meta.url)),
    "utf8",
  ),
) as Record<string, unknown[][]>;

/** The schema of each handler's argument list. */
const outputEventSchemas: Record<string, z.ZodTypeAny> = {
  showGeneratedContent: ShowGeneratedArgsSchema,
  // These handlers take one argument, an object.
  saveResult: SaveResultSchema,
  outputSizeNotice: OutputSizeNoticeSchema,
};
const takesOneObject = new Set(["saveResult", "outputSizeNotice"]);

describe("[IPC Contracts] host calls of the generated output", () => {
  it("knows exactly the handlers the host calls", () => {
    expect(Object.keys(outputEvents).sort()).toEqual(
      Object.keys(outputEventSchemas).sort(),
    );
  });

  it("accepts every example call unchanged", () => {
    for (const [handler, calls] of Object.entries(outputEvents)) {
      expect(calls.length, handler).toBeGreaterThan(0);
      for (const args of calls) {
        const label = `${handler} ${JSON.stringify(args)}`;
        let input: unknown = args;
        if (takesOneObject.has(handler)) {
          expect(args, label).toHaveLength(1);
          input = args[0];
        }
        const result = outputEventSchemas[handler].safeParse(input);
        expect(result.error?.issues ?? [], label).toEqual([]);
        // Nothing the host sends is dropped by the schema.
        expect(result.data, label).toEqual(input);
      }
    }
  });

  it("covers every status of a save and of a size notice on both sides", () => {
    const statuses = (handler: string) =>
      new Set(
        outputEvents[handler].map(
          ([argument]) => (argument as { status: string }).status,
        ),
      );
    expect(statuses("saveResult")).toEqual(
      new Set(SaveResultSchema.options.map((o) => o.shape.status.value)),
    );
    expect(statuses("outputSizeNotice")).toEqual(
      new Set(OutputSizeNoticeSchema.options.map((o) => o.shape.status.value)),
    );
  });
});
