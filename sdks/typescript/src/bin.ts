export function resolveStacklessBin(explicit?: string): string {
  if (explicit !== undefined && explicit.length > 0) {
    return explicit;
  }
  const fromEnv = process.env.STACKLESS_BIN;
  if (fromEnv !== undefined && fromEnv.length > 0) {
    return fromEnv;
  }
  return "stackless";
}
