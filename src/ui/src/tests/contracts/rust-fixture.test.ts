/* @vitest-environment node */

// Cross-language contract test.
//
// `fixtures/ui_state_window.json` is written by the Rust backend
// (src/app/view_model/contract_tests.rs) and compared there against what
// `generate_ui_state` really serializes. Here the same file must pass the full
// runtime schema, so Rust and TypeScript cannot drift apart without CI failing
// on one of the two sides.
//
// Regenerate after an intended change: `UPDATE_CONTRACT_FIXTURES=1 cargo test contract_fixture`

import { describe, it, expect, vi, afterEach } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  AnyCommandMessageSchema,
  CommandSchemas,
  ConfigOptionSchema,
  OutputSizeNoticeSchema,
  IgnorePathRefusalSchema,
  SaveFilePayloadSchema,
  SaveResultSchema,
  ShowGeneratedArgsSchema,
  UiConfigSchema,
  UiStateSchema,
  TreeRowSchema,
  type CommandName,
} from "$lib/ipc/schema";
import type { z } from "zod";
import type { TreeRow } from "$lib/types";

const fixturePath = fileURLToPath(
  new URL("./fixtures/ui_state_window.json", import.meta.url),
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

function find(rows: TreeRow[], path: string): TreeRow {
  const hit = rows.find((row) => row.path === path);
  if (!hit) throw new Error(`${path} is not part of the fixture`);
  return hit;
}

describe("[IPC Contracts] UiState serialized by the Rust backend", () => {
  it("passes the full UiStateSchema", () => {
    const result = UiStateSchema.safeParse(fixture);
    expect(result.error?.issues ?? []).toEqual([]);
    expect(result.success).toBe(true);
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

  it("sends exactly the fields of a row the schema knows", () => {
    // TreeRowSchema is passthrough as well; compare the key sets explicitly.
    const known = Object.keys(TreeRowSchema.shape).sort();
    const { rows } = fixture as { rows: Record<string, unknown>[] };
    expect(rows.length).toBeGreaterThan(0);
    for (const row of rows) {
      expect(Object.keys(row).sort(), String(row.path)).toEqual(known);
    }
  });

  it("is a window of the list of rows, not the list", () => {
    const state = UiStateSchema.parse(fixture);

    expect(state.rows_total).toBe(9);
    expect(state.rows_start).toBe(1);
    expect(state.rows.map((row) => row.name)).toEqual([
      "empty",
      "src",
      "core",
      "lib.rs",
      "main.rs",
      "vendor",
      "README.md",
      "logo.png",
    ]);
    // The first row of the list is above the window.
    expect(state.rows.some((row) => row.path === "/project/docs")).toBe(false);
    expect(state.rows_revision).toBeGreaterThan(0);
  });

  it("states the nesting level of every row", () => {
    const { rows } = UiStateSchema.parse(fixture);
    expect(rows.map((row) => row.depth)).toEqual([0, 0, 1, 1, 1, 0, 0, 0]);
  });

  it("answers the row lookup with a row of the whole list", () => {
    const state = UiStateSchema.parse(fixture);
    expect(state.located).toEqual({ request: 7, row: 5 });
    expect(state.rows[5 - state.rows_start].path).toBe("/project/src/main.rs");
  });

  it("describes every kind of row completely", () => {
    const { rows } = UiStateSchema.parse(fixture);

    expect(find(rows, "/project/src")).toMatchObject({
      is_directory: true,
      is_expanded: true,
      has_children: true,
      selection_state: "partial",
    });
    // Collapsed inside an expanded directory: its files are no rows, and all
    // of them are selected.
    expect(find(rows, "/project/src/core")).toMatchObject({
      is_expanded: false,
      has_children: true,
      children_loaded: true,
      selection_state: "full",
    });
    expect(rows.some((row) => row.path.startsWith("/project/src/core/"))).toBe(
      false,
    );
    expect(find(rows, "/project/vendor")).toMatchObject({
      has_children: false,
      children_loaded: false,
    });
    expect(find(rows, "/project/empty")).toMatchObject({
      has_children: false,
      children_loaded: true,
    });
    expect(find(rows, "/project/src/main.rs")).toMatchObject({
      is_directory: false,
      is_match: true,
      is_previewed: true,
      selection_state: "full",
    });
    expect(find(rows, "/project/logo.png")).toMatchObject({
      is_binary: true,
      size: 65536,
    });
  });

  it("carries the totals of the complete tree, not of the window", () => {
    const state = UiStateSchema.parse(fixture);

    expect(state.tree_files_count).toBe(7);
    expect(state.tree_dirs_count).toBe(5);
    expect(state.rows.filter((row) => !row.is_directory).length).toBeLessThan(
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

  it("refuses more than a command in a message, like the host", () => {
    const message = { command: "selectAll", payload: null };
    expect(AnyCommandMessageSchema.safeParse(message).success).toBe(true);
    for (const extra of [{ extra: 1 }, { seq: 1 }]) {
      expect(
        AnyCommandMessageSchema.safeParse({ ...message, ...extra }).success,
        JSON.stringify(extra),
      ).toBe(false);
    }
  });

  it("covers every kind of row lookup on both sides", () => {
    const kindsOf = (targets: { kind: string }[]) =>
      new Set(targets.map((target) => target.kind));
    const hostKinds = kindsOf(
      commandFixture.locateRow.map(
        (payload) => (payload as { target: { kind: string } }).target,
      ),
    );
    const uiKinds = new Set(
      CommandSchemas.locateRow.shape.target.options.map(
        (option) => option.shape.kind.value,
      ),
    );
    expect(hostKinds).toEqual(uiKinds);
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
  ignorePathRefused: IgnorePathRefusalSchema,
};
const takesOneObject = new Set([
  "saveResult",
  "outputSizeNotice",
  "ignorePathRefused",
]);

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

  it("covers every reason the ignore button is refused for on both sides", () => {
    const reasons = new Set(
      outputEvents.ignorePathRefused.map(
        ([argument]) => (argument as { reason: string }).reason,
      ),
    );
    expect(reasons).toEqual(
      new Set(IgnorePathRefusalSchema.options.map((o) => o.shape.reason.value)),
    );
  });
});

// The envelope: `fixtures/command_messages.json` holds the same examples as
// complete messages, numbered from 1 in the order they are listed, exactly as
// the host reads them (it checks that itself: each one is read back as the
// command it was written from). Here `post()` has to write these messages.
const messageFixture = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("./fixtures/command_messages.json", import.meta.url)),
    "utf8",
  ),
) as { seq: number; command: CommandName; payload: unknown }[];

describe("[IPC Contracts] messages read by the Rust backend", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  /** Posts every example with a `post()` that has not posted anything yet. */
  async function postExamples() {
    const posted: string[] = [];
    vi.resetModules();
    vi.stubGlobal("window", {
      ipc: { postMessage: (message: string) => posted.push(message) },
    });
    const { post } = await import("$lib/services/backend");
    const send = post as (command: CommandName, payload: unknown) => number;
    const returned = messageFixture.map(({ command, payload }) =>
      send(command, payload),
    );
    return { posted, returned };
  }

  it("covers every command", () => {
    expect(new Set(messageFixture.map((message) => message.command))).toEqual(
      new Set(Object.keys(CommandSchemas)),
    );
    expect(messageFixture.map((message) => message.seq)).toEqual(
      messageFixture.map((_, index) => index + 1),
    );
  });

  for (const [build, dev] of [
    ["a development build (validated)", true],
    ["a production build", false],
  ] as const) {
    it(`are what post() writes in ${build}`, async () => {
      vi.stubEnv("DEV", dev);
      const { posted, returned } = await postExamples();

      expect(posted.map((message) => JSON.parse(message) as unknown)).toEqual(
        messageFixture,
      );
      // The number of a command is what `post()` returns for it.
      expect(returned).toEqual(messageFixture.map((message) => message.seq));
      // An envelope is a number, a command and its payload, nothing else.
      for (const message of posted) {
        expect(Object.keys(JSON.parse(message) as object).sort()).toEqual([
          "command",
          "payload",
          "seq",
        ]);
      }
    });
  }
});
