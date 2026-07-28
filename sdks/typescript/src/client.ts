import { spawnSync } from "node:child_process";

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
      lease?: string;
      confirmPaid?: boolean;
    }
  | {
      kind: "resume";
      name: string;
      file?: string;
      sources?: string[];
      dirty?: boolean;
      lease?: string;
    };

export type UpOutcome = {
  instance: string;
  substrate: string;
  origins: Record<string, string>;
  integrations: Record<string, Record<string, string>>;
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
) => SpawnResult;

export type ClientOptions = {
  bin?: string;
  cwd?: string;
  run?: SpawnRunner;
};

function defaultRun(
  bin: string,
  args: string[],
  options: { cwd?: string },
): SpawnResult {
  const result = spawnSync(bin, args, {
    cwd: options.cwd,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  return {
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    status: result.status,
  };
}

type OriginEntry = { service: string; origin: string };

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
  const integrations =
    (raw.integrations as Record<string, Record<string, string>> | undefined) ??
    {};
  return {
    instance: String(raw.instance),
    substrate: String(raw.substrate),
    origins,
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
  if (request.dirty) {
    args.push("--dirty");
  }
  if (request.lease !== undefined) {
    args.push("--lease", request.lease);
  }
  return args;
}

/**
 * Blocking CLI subprocess per call (`spawnSync`). Methods are async only for
 * a stable Promise-based surface; work runs synchronously on the caller thread.
 */
export class Client {
  private readonly bin: string;
  private readonly cwd?: string;
  private readonly run: SpawnRunner;

  constructor(options: ClientOptions = {}) {
    this.bin = resolveStacklessBin(options.bin);
    this.cwd = options.cwd;
    this.run = options.run ?? defaultRun;
  }

  static system(options?: { bin?: string; cwd?: string }): Client {
    return new Client(options);
  }

  async up(request: UpRequest): Promise<UpOutcome> {
    const raw = this.invoke(buildUpArgs(request));
    return mapUpOutcome(raw);
  }

  async down(name: string): Promise<DownOutcome> {
    const raw = this.invoke(["down", name]);
    return mapDownOutcome(raw);
  }

  async verify(name: string, tier?: string): Promise<VerifyOutcome> {
    const args = ["verify", name];
    if (tier !== undefined) {
      args.push("--tier", tier);
    }
    const raw = this.invoke(args);
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

  /** Resolved stackless binary path (for tests and tooling). */
  resolvedBin(): string {
    return this.bin;
  }

  private invoke(subcommandArgs: string[]): Record<string, unknown> {
    const args = ["--json", ...subcommandArgs];
    const result = this.run(this.bin, args, { cwd: this.cwd });
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
