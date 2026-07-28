export class StacklessError extends Error {
  readonly code?: string;

  constructor(message: string, code?: string) {
    super(message);
    this.name = "StacklessError";
    this.code = code;
  }
}

type ErrorBody = {
  code?: string;
  message?: string;
};

export function parseEnvelope<T extends Record<string, unknown>>(
  stdout: string,
): T {
  let parsed: unknown;
  try {
    parsed = JSON.parse(stdout.trim());
  } catch {
    throw new StacklessError("stackless CLI returned invalid JSON");
  }
  if (typeof parsed !== "object" || parsed === null) {
    throw new StacklessError("stackless CLI returned invalid envelope");
  }
  const envelope = parsed as Record<string, unknown>;
  if (envelope.ok === false) {
    const err = envelope.error as ErrorBody | undefined;
    throw new StacklessError(
      err?.message ?? "stackless command failed",
      err?.code,
    );
  }
  if (envelope.ok !== true) {
    throw new StacklessError("stackless CLI envelope missing ok: true");
  }
  return envelope as T;
}
