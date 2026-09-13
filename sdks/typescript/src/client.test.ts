import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { resolveStacklessBin } from "./bin.js";
import { Client, endpointUrls, StacklessError, type SpawnRunner } from "./client.js";
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

  it("forwards the controller to submissions and operation reads", async () => {
    const calls: string[][] = [];
    const run: SpawnRunner = (_bin, args) => {
      calls.push(args);
      return { status: 0, stderr: "", stdout: JSON.stringify({ ok: true, operation: { id: "op" }, result: { operation: { id: "op" }, events: [] } }) };
    };
    const client = new Client({ bin: "/fake/stackless", controller: "ssh://deploy@builder", run });
    await client.submitDown("demo");
    await client.operation("op");
    for (const args of calls) expect(args.slice(0, 3)).toEqual(["--json", "--controller", "ssh://deploy@builder"]);
  });

  it.each([false, true])("requires an explicit caller host grant: %s", async (allowed) => {
    const run: SpawnRunner = (_bin, args) => {
      expect(args.includes("--allow-host-execution")).toBe(allowed);
      return { status: 0, stderr: "", stdout: JSON.stringify({ ok: true, instance: "demo", instance_id: "owner-1", substrate: "local", origins: [] }) };
    };
    const client = new Client({ bin: "/fake/stackless", run });
    await client.up({ kind: "create", on: "local", allowHostExecution: allowed });
    await client.up({ kind: "resume", name: "demo", allowHostExecution: allowed });
  });

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
          instance_id: "owner-1",
          substrate: "local",
          origins: [
            { service: "web", origin: "http://demo.localhost:4444/" },
            { service: "api", origin: "http://api.demo.localhost:4444/" },
          ],
          placements: { workloads: { web: "local", api: "fly" }, resources: { clerk: "local" } },
          endpoints: { public: { workload: "web", url: "https://public.example.test", source: "declared" } },
          integrations: {
            clerk: { secret_key: { kind: "secret_ref", instance_id: "owner-1", integration: "clerk", output: "secret_key" } },
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
    expect(outcome.endpoints.public.source).toBe("declared");
    expect(endpointUrls(outcome)).toEqual({ public: "https://public.example.test" });
    expect(outcome.substrate).toBe("local");
    expect(outcome.placements.workloads.api).toBe("fly");
    expect(outcome.placements.resources.clerk).toBe("local");
    expect(outcome.origins).toEqual({
      web: "http://demo.localhost:4444/",
      api: "http://api.demo.localhost:4444/",
    });
    expect(outcome.integrations.clerk.secret_key).toEqual({ kind: "secret_ref", instance_id: "owner-1", integration: "clerk", output: "secret_key" });
  });

  it("defaults integrations to {}", async () => {
    const run: SpawnRunner = () => ({
      status: 0,
      stderr: "",
      stdout: JSON.stringify({
        ok: true,
        instance: "x",
        instance_id: "owner-1",
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


describe("async process transport", () => {
  it("allows timers to run while the CLI is executing", async () => {
    const dir = mkdtempSync(join(tmpdir(), "stackless-async-"));
    const bin = join(dir, "stackless");
    writeFileSync(bin, `#!${process.execPath}\nsetTimeout(() => console.log(JSON.stringify({ok:true})), 100);\n`);
    chmodSync(bin, 0o755);
    try {
      let timerFired = false;
      setTimeout(() => { timerFired = true; }, 0);
      const waiting = new Client({ bin }).check("unused.toml");
      await waiting;
      expect(timerFired).toBe(true);
    } finally { rmSync(dir, { recursive: true, force: true }); }
  });
});

for (const ref of ["sk_test_CANARY", {kind:"secret_ref", instance_id:"foreign", integration:"clerk", output:"secret_key"}]) {
  it("rejects plaintext and foreign secret references", async () => {
    const client = new Client({run: () => ({status:0, stderr:"", stdout:JSON.stringify({ok:true, instance:"demo", instance_id:"owner-1", substrate:"local", integrations:{clerk:{secret_key:ref}}})})});
    await expect(client.up({kind:"create", on:"local"})).rejects.toThrow("invalid or foreign integration secret reference");
  });
}

it.each([[], "url", { public: { workload: "web", url: "https://example.test", source: "invented" } }, { public: { url: "https://example.test", source: "provider" } }])("rejects malformed endpoint bindings: %j", async (endpoints) => {
  const client = new Client({ bin: "/fake/stackless", run: () => ({ status: 0, stderr: "", stdout: JSON.stringify({ ok: true, instance: "demo", instance_id: "owner-1", substrate: "local", endpoints }) }) });
  await expect(client.up({ kind: "resume", name: "demo" })).rejects.toThrow("invalid endpoint binding");
});

it("binds endpoint URL maps through generated TypeScript names", async () => {
  const { bindEndpoints, BindError } = await import("../../../crates/stackless-idl/testdata/endpoints.js");
  const client = new Client({ bin: "/fake/stackless", run: () => ({ status: 0, stderr: "", stdout: JSON.stringify({
    ok: true, instance: "demo", instance_id: "owner-1", substrate: "local", endpoints: {
      "native-api": { workload: "web", url: "http://native.example.test", source: "provider" },
      "public-api": { workload: "web", url: "https://public.example.test/v1", source: "declared" },
    },
  }) }) });
  const outcome = await client.up({ kind: "resume", name: "demo" });
  expect(bindEndpoints(endpointUrls(outcome))).toEqual({ nativeApi: "http://native.example.test", publicApi: "https://public.example.test/v1" });
  expect(() => bindEndpoints({})).toThrow(BindError);
});

it.each([[], "local", {}, { workloads: { api: false }, resources: {} }, { workloads: {}, resources: { db: "" } }])("rejects malformed placements: %j", async (placements) => {
  const run: SpawnRunner = () => ({ status: 0, stderr: "", stdout: JSON.stringify({ ok: true, instance: "demo", instance_id: "owner-1", substrate: "local", origins: [], placements }) });
  await expect(new Client({ bin: "/fake/stackless", run }).up({ kind: "create", on: "local" })).rejects.toThrow(/invalid .*placement/);
});
