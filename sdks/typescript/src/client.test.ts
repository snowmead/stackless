import { afterEach, describe, expect, it } from "vitest";

import { resolveStacklessBin } from "./bin.js";
import { Client, StacklessError, type SpawnRunner } from "./client.js";
import { parseEnvelope } from "./envelope.js";

describe("parseEnvelope", () => {
  it("throws StacklessError with code on failure envelope", () => {
    const stdout = JSON.stringify({
      ok: false,
      error: { code: "instance_not_found", message: "missing demo" },
    });
    try {
      parseEnvelope(stdout);
      expect.unreachable("expected throw");
    } catch (err) {
      expect(err).toBeInstanceOf(StacklessError);
      expect((err as StacklessError).code).toBe("instance_not_found");
      expect((err as StacklessError).message).toBe("missing demo");
    }
  });
});

describe("Client", () => {
  const envBin = process.env.STACKLESS_BIN;

  afterEach(() => {
    if (envBin === undefined) {
      delete process.env.STACKLESS_BIN;
    } else {
      process.env.STACKLESS_BIN = envBin;
    }
  });

  it("maps up origins and integrations", async () => {
    const run: SpawnRunner = (_bin, args) => {
      expect(args[0]).toBe("--json");
      expect(args[1]).toBe("up");
      expect(args).toContain("--on");
      expect(args).toContain("local");
      return {
        status: 0,
        stderr: "",
        stdout: JSON.stringify({
          schema_version: 1,
          ok: true,
          instance: "demo",
          substrate: "local",
          origins: [
            { service: "web", origin: "http://demo.localhost:4444/" },
            { service: "api", origin: "http://api.demo.localhost:4444/" },
          ],
          integrations: {
            clerk: { secret_key: "sk_test", publishable_key: "pk_test" },
          },
        }),
      };
    };

    const client = new Client({ bin: "/usr/bin/stackless", run });
    const outcome = await client.up({
      kind: "create",
      name: "demo",
      on: "local",
    });

    expect(outcome.instance).toBe("demo");
    expect(outcome.substrate).toBe("local");
    expect(outcome.origins).toEqual({
      web: "http://demo.localhost:4444/",
      api: "http://api.demo.localhost:4444/",
    });
    expect(outcome.integrations.clerk.secret_key).toBe("sk_test");
  });

  it("defaults integrations to {}", async () => {
    const run: SpawnRunner = () => ({
      status: 0,
      stderr: "",
      stdout: JSON.stringify({
        ok: true,
        instance: "x",
        substrate: "local",
        origins: [],
      }),
    });
    const client = new Client({ run });
    const outcome = await client.up({ kind: "create", on: "local" });
    expect(outcome.integrations).toEqual({});
  });

  it("propagates CLI error envelopes from up", async () => {
    const run: SpawnRunner = () => ({
      status: 1,
      stderr: "boom",
      stdout: JSON.stringify({
        ok: false,
        error: { code: "bad_argument", message: "invalid lease" },
      }),
    });
    const client = new Client({ run });
    await expect(
      client.up({ kind: "create", on: "local" }),
    ).rejects.toMatchObject({
      code: "bad_argument",
      message: "invalid lease",
    });
  });
});

describe("resolveStacklessBin", () => {
  const previous = process.env.STACKLESS_BIN;

  afterEach(() => {
    if (previous === undefined) {
      delete process.env.STACKLESS_BIN;
    } else {
      process.env.STACKLESS_BIN = previous;
    }
  });

  it("prefers STACKLESS_BIN over default", () => {
    process.env.STACKLESS_BIN = "/custom/stackless";
    expect(resolveStacklessBin()).toBe("/custom/stackless");
    expect(new Client().resolvedBin()).toBe("/custom/stackless");
  });

  it("prefers explicit bin over STACKLESS_BIN", () => {
    process.env.STACKLESS_BIN = "/from-env";
    expect(resolveStacklessBin("/explicit")).toBe("/explicit");
    expect(new Client({ bin: "/explicit" }).resolvedBin()).toBe("/explicit");
  });
});
