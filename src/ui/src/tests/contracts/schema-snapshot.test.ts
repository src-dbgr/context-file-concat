import { describe, it, expect } from "vitest";
import { toJSONSchema } from "zod";
import {
  AnyCommandMessageSchema,
  UiStateSchema,
  ConfigSchema,
} from "$lib/ipc/schema";

const norm = (x: unknown) => JSON.parse(JSON.stringify(x));

describe("[IPC Contracts] JSON schema snapshots", () => {
  it("AnyCommandMessage schema snapshot", () => {
    expect(norm(toJSONSchema(AnyCommandMessageSchema))).toMatchSnapshot();
  });
  it("UiState schema snapshot", () => {
    expect(norm(toJSONSchema(UiStateSchema))).toMatchSnapshot();
  });
  it("Config schema snapshot", () => {
    expect(norm(toJSONSchema(ConfigSchema))).toMatchSnapshot();
  });
});
