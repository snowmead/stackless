import { spawn } from "node:child_process";

import { resolveStacklessBin } from "./bin.js";
import { parseEnvelope, StacklessError } from "./envelope.js";

export type UpRequest =
  | {
      kind: "create";
      name?: string;
      file?: string;
      on: string;
      sources?: string[];
      dirty?: boolean;
      allowHostExecution?: boolean;
      lease?: string;
      confirmPaid?: boolean;
    }
  | {
      kind: "resume";
      name: string;
      file?: string;
      sources?: string[];
      dirty?: boolean;
      allowHostExecution?: boolean;
      lease?: string;
    };

export type SecretRef = {
  kind: "secret_ref";
  instance_id: string;
  integration: string;
  output: string;
};

export type EndpointBinding = {
  workload: string;
  url: string;
  source: "provider" | "declared";
};

export function endpointUrls(outcome: UpOutcome): Record<string, string> {
  return Object.fromEntries(Object.entries(outcome.endpoints).map(([name, endpoint]) => [name, endpoint.url]));
}

export type Placements = {
  workloads: Record<string, string>;
  resources: Record<string, string>;
};

export type UpOutcome = {
  instance_id: string;
  instance: string;
  substrate: string;
  origins: Record<string, string>;
  endpoints: Record<string, EndpointBinding>;
  placements: Placements;
  integrations: Record<string, Record<string, SecretRef>>;
};

export type DownOutcome = {
  instance: string;
  status: "destroyed" | "already_down";
};

export type VerifyOutcome = {
  instance: string;
  tier?: string;
  duration_ms: number;
  exit_status: number;
  log_path: string;
  lease_remaining_secs?: number;
};

export type SpawnResult = {
  stdout: string;
  stderr: string;
  status: number | null;
};

export type SpawnRunner = (
  bin: string,
  args: string[],
  options: { cwd?: string },
) => SpawnResult | Promise<SpawnResult>;

export type ClientOptions = {
  bin?: string;
  cwd?: string;
  controller?: string;
  run?: SpawnRunner;
};

function defaultRun(bin: string, args: string[], options: { cwd?: string }): Promise<SpawnResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(bin, args, { cwd: options.cwd, stdio: ["ignore", "pipe", "pipe"] });
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let size = 0;
    const collect = (target: Buffer[]) => (chunk: Buffer) => {
      size += chunk.length;
      if (size > 64 * 1024 * 1024) {
        child.kill();
        reject(new StacklessError("stackless CLI output exceeded 64 MiB"));
      } else { target.push(chunk); }
    };
    child.stdout.on("data", collect(stdout));
    child.stderr.on("data", collect(stderr));
    child.on("error", reject);
    child.on("close", (status) => resolve({ stdout: Buffer.concat(stdout).toString("utf8"), stderr: Buffer.concat(stderr).toString("utf8"), status }));
  });
}

export type Operation = {
  id: string;
  instance: string;
  verb: "up" | "down" | "verify" | "gc";
  status: "queued" | "running" | "succeeded" | "failed" | "cancelled" | "interrupted";
  result: unknown;
  error: unknown;
  cancel_requested: boolean;
  created_at: number;
  updated_at: number;
};
export type OperationPage = { operation: Operation; events: { sequence: number; event: unknown }[] };

type OriginEntry = { service: string; origin: string };

function mapPlacements(raw: unknown): Placements {
  if (raw == null) return { workloads: {}, resources: {} };
  if (typeof raw !== "object" || Array.isArray(raw)) throw new StacklessError("invalid placements");
  const result: Placements = { workloads: {}, resources: {} };
  for (const kind of ["workloads", "resources"] as const) {
    const values = (raw as Record<string, unknown>)[kind];
    if (!values || typeof values !== "object" || Array.isArray(values)) throw new StacklessError("invalid placement map");
    for (const [name, on] of Object.entries(values)) {
      if (!name || typeof on !== "string" || !on) throw new StacklessError("invalid hosting placement");
      result[kind][name] = on;
    }
  }
  return result;
}

function mapUpOutcome(raw: Record<string, unknown>): UpOutcome {
  const origins: Record<string, string> = {};
  const list = raw.origins;
  if (Array.isArray(list)) {
    for (const item of list as OriginEntry[]) {
      if (item?.service && item?.origin) {
        origins[item.service] = item.origin;
      }
    }
  }
  const instance_id = raw.instance_id;
  if (typeof instance_id !== "string" || !instance_id) {
    throw new StacklessError("up response lacks an immutable instance ID");
  }
  const integrations: UpOutcome["integrations"] = {};
  const source = raw.integrations ?? {};
  if (!source || typeof source !== "object" || Array.isArray(source)) {
    throw new StacklessError("invalid integration reference map");
  }
  for (const [integration, outputs] of Object.entries(source)) {
    if (!outputs || typeof outputs !== "object" || Array.isArray(outputs)) {
      throw new StacklessError("invalid integration reference map");
    }
    integrations[integration] = {};
    for (const [output, rawRef] of Object.entries(outputs)) {
      const ref = rawRef as SecretRef | null;
      if (!ref || typeof ref !== "object" || ref.kind !== "secret_ref" ||
          ref.instance_id !== instance_id || ref.integration !== integration || ref.output !== output ||
          Object.keys(ref).sort().join(",") !== "instance_id,integration,kind,output") {
        throw new StacklessError("invalid or foreign integration secret reference");
      }
      integrations[integration][output] = { kind: "secret_ref", instance_id, integration, output };
    }
  }
  const endpoints: Record<string, EndpointBinding> = {};
  const bindings = raw.endpoints ?? {};
  if (typeof bindings !== "object" || Array.isArray(bindings)) {
    throw new StacklessError("invalid endpoint binding map");
  }
  for (const [name, value] of Object.entries(bindings)) {
    const binding = value as EndpointBinding | null;
    if (!binding || typeof binding.workload !== "string" || !binding.workload ||
        typeof binding.url !== "string" || !binding.url ||
        (binding.source !== "provider" && binding.source !== "declared")) {
      throw new StacklessError("invalid endpoint binding");
    }
    endpoints[name] = { workload: binding.workload, url: binding.url, source: binding.source };
  }
  return {
    instance: String(raw.instance),
    instance_id,
    substrate: String(raw.substrate),
    origins,
    endpoints,
    placements: mapPlacements(raw.placements),
    integrations,
  };
}

function mapDownOutcome(raw: Record<string, unknown>): DownOutcome {
  const status = String(raw.outcome ?? "destroyed");
  if (status !== "destroyed" && status !== "already_down") {
    throw new StacklessError(`unexpected down outcome: ${status}`);
  }
  return {
    instance: String(raw.instance),
    status,
  };
}

function mapVerifyOutcome(raw: Record<string, unknown>): VerifyOutcome {
  const out: VerifyOutcome = {
    instance: String(raw.instance),
    duration_ms: Number(raw.duration_ms),
    exit_status: Number(raw.exit_status),
    log_path: String(raw.log_path),
  };
  if (raw.tier !== undefined && raw.tier !== null) {
    out.tier = String(raw.tier);
  }
  if (raw.lease_remaining_secs !== undefined && raw.lease_remaining_secs !== null) {
    out.lease_remaining_secs = Number(raw.lease_remaining_secs);
  }
  return out;
}

function buildUpArgs(request: UpRequest): string[] {
  const args = ["up"];
  if (request.kind === "create") {
    if (request.name !== undefined) {
      args.push("--name", request.name);
    }
    if (request.file !== undefined) {
      args.push("--file", request.file);
    }
    args.push("--on", request.on);
    if (request.confirmPaid) {
      args.push("--confirm-paid");
    }
  } else {
    args.push("--name", request.name);
    if (request.file !== undefined) {
      args.push("--file", request.file);
    }
  }
  if (request.sources !== undefined) {
    for (const source of request.sources) {
      args.push("--source", source);
    }
  }
  if (request.allowHostExecution) {
    args.push("--allow-host-execution");
  }
  if (request.dirty) {
    args.push("--dirty");
  }
  if (request.lease !== undefined) {
    args.push("--lease", request.lease);
  }
  return args;
}

/**
 * Async CLI transport. The controller keeps operations alive when this client exits.
 */
export class Client {
  private readonly bin: string;
  private readonly cwd?: string;
  private readonly controller?: string;
  private readonly run: SpawnRunner;

  constructor(options: ClientOptions = {}) {
    this.bin = resolveStacklessBin(options.bin);
    this.cwd = options.cwd;
    this.controller = options.controller;
    this.run = options.run ?? defaultRun;
  }

  static system(options?: { bin?: string; cwd?: string; controller?: string }): Client {
    return new Client(options);
  }

  async up(request: UpRequest): Promise<UpOutcome> {
    const raw = await this.invoke(buildUpArgs(request));
    return mapUpOutcome(raw);
  }

  async down(name: string): Promise<DownOutcome> {
    const raw = await this.invoke(["down", name]);
    return mapDownOutcome(raw);
  }

  async verify(name: string, tier?: string): Promise<VerifyOutcome> {
    const args = ["verify", name];
    if (tier !== undefined) {
      args.push("--tier", tier);
    }
    const raw = await this.invoke(args);
    return mapVerifyOutcome(raw);
  }

  async status(name: string): Promise<unknown> {
    return this.invoke(["status", name]);
  }

  async list(): Promise<unknown> {
    return this.invoke(["list"]);
  }

  async logs(
    name: string,
    opts?: { service?: string; tail?: number },
  ): Promise<unknown> {
    const args = ["logs", name];
    if (opts?.service !== undefined) {
      args.push(opts.service);
    }
    if (opts?.tail !== undefined) {
      args.push("--tail", String(opts.tail));
    }
    return this.invoke(args);
  }

  async check(file: string, on?: string): Promise<unknown> {
    const args = ["check", file];
    if (on !== undefined) {
      args.push("--on", on);
    }
    return this.invoke(args);
  }

  async submitUp(request: UpRequest): Promise<Operation> {
    const raw = await this.invoke([...buildUpArgs(request), "--no-wait"]);
    return raw.operation as Operation;
  }

  async submitDown(name: string): Promise<Operation> {
    const raw = await this.invoke(["down", name, "--no-wait"]);
    return raw.operation as Operation;
  }

  async operation(id: string, after = 0): Promise<OperationPage> {
    const raw = await this.invoke(["operation", "get", id, "--after", String(after)]);
    return raw.result as OperationPage;
  }

  async cancelOperation(id: string): Promise<Operation> {
    const raw = await this.invoke(["operation", "cancel", id]);
    return raw.operation as Operation;
  }

  async waitOperation<T = unknown>(id: string): Promise<T> {
    const raw = await this.invoke(["operation", "wait", id]);
    return raw.result as T;
  }

  async operations(instance?: string): Promise<Operation[]> {
    const args = ["operation", "list"];
    if (instance !== undefined) args.push("--instance", instance);
    return (await this.invoke(args)).result as Operation[];
  }

  /** Resolved stackless binary path (for tests and tooling). */
  resolvedBin(): string {
    return this.bin;
  }

  private async invoke(subcommandArgs: string[]): Promise<Record<string, unknown>> {
    const args = ["--json", ...(this.controller ? ["--controller", this.controller] : []), ...subcommandArgs];
    const result = await this.run(this.bin, args, { cwd: this.cwd });
    const stdout = result.stdout.trim();
    if (stdout.length > 0) {
      try {
        return parseEnvelope(stdout);
      } catch (err) {
        if (err instanceof StacklessError) {
          throw err;
        }
      }
    }
    if (result.status !== 0) {
      const detail = result.stderr.trim() || `exit status ${result.status}`;
      throw new StacklessError(detail);
    }
    if (stdout.length === 0) {
      throw new StacklessError("stackless CLI returned empty stdout");
    }
    return parseEnvelope(stdout);
  }
}

export { StacklessError } from "./envelope.js";
export { resolveStacklessBin } from "./bin.js";
