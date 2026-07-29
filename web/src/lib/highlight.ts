import { createElement, type ReactNode } from "react";

type TokenClass =
  | "t-header"
  | "t-key"
  | "t-punct"
  | "t-str"
  | "t-interp"
  | "t-bool"
  | "t-comment"
  | "t-num";

function tok(cls: TokenClass, text: string, key: string | number): ReactNode {
  return createElement("span", { key, className: cls }, text);
}

function highlightString(raw: string, keyBase: string): ReactNode[] {
  const out: ReactNode[] = [];
  const re = /\$\{[^}]+\}/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let i = 0;
  while ((m = re.exec(raw)) !== null) {
    if (m.index > last) {
      out.push(tok("t-str", raw.slice(last, m.index), `${keyBase}-s${i++}`));
    }
    out.push(tok("t-interp", m[0], `${keyBase}-i${i++}`));
    last = m.index + m[0].length;
  }
  if (last < raw.length) {
    out.push(tok("t-str", raw.slice(last), `${keyBase}-s${i++}`));
  }
  return out;
}

function highlightTomlValue(value: string, keyBase: string): ReactNode[] {
  const out: ReactNode[] = [];
  let i = 0;
  let part = 0;

  while (i < value.length) {
    const ch = value[i]!;

    if (/\s/.test(ch)) {
      let j = i + 1;
      while (j < value.length && /\s/.test(value[j]!)) j++;
      out.push(value.slice(i, j));
      i = j;
      continue;
    }

    if (ch === '"' || ch === "'") {
      const quote = ch;
      let j = i + 1;
      while (j < value.length) {
        if (value[j] === "\\" && j + 1 < value.length) {
          j += 2;
          continue;
        }
        if (value[j] === quote) {
          j++;
          break;
        }
        j++;
      }
      out.push(...highlightString(value.slice(i, j), `${keyBase}-q${part++}`));
      i = j;
      continue;
    }

    if ("{}[],=".includes(ch)) {
      out.push(tok("t-punct", ch, `${keyBase}-p${part++}`));
      i++;
      continue;
    }

    if (/[0-9]/.test(ch)) {
      let j = i + 1;
      while (j < value.length && /[0-9.]/.test(value[j]!)) j++;
      out.push(tok("t-num", value.slice(i, j), `${keyBase}-n${part++}`));
      i = j;
      continue;
    }

    if (/[A-Za-z_]/.test(ch)) {
      let j = i + 1;
      while (j < value.length && /[A-Za-z0-9_.-]/.test(value[j]!)) j++;
      const word = value.slice(i, j);
      if (word === "true" || word === "false") {
        out.push(tok("t-bool", word, `${keyBase}-b${part++}`));
      } else {
        out.push(tok("t-key", word, `${keyBase}-k${part++}`));
      }
      i = j;
      continue;
    }

    out.push(tok("t-punct", ch, `${keyBase}-x${part++}`));
    i++;
  }

  return out;
}

function highlightTomlLine(line: string, lineKey: number): ReactNode {
  const trimmed = line.trimStart();
  const indent = line.slice(0, line.length - trimmed.length);

  if (trimmed === "") {
    return createElement("span", {
      key: lineKey,
      className: "toml-line toml-line-empty",
    });
  }

  if (trimmed.startsWith("#")) {
    return createElement(
      "span",
      { key: lineKey, className: "toml-line" },
      indent,
      tok("t-comment", trimmed, `${lineKey}-c`),
    );
  }

  if (trimmed.startsWith("[")) {
    return createElement(
      "span",
      { key: lineKey, className: "toml-line" },
      indent,
      tok("t-header", trimmed, `${lineKey}-h`),
    );
  }

  const eq = trimmed.indexOf("=");
  if (eq === -1) {
    return createElement(
      "span",
      { key: lineKey, className: "toml-line" },
      indent,
      tok("t-key", trimmed, `${lineKey}-k`),
    );
  }

  const key = trimmed.slice(0, eq).trimEnd();
  const afterKey = trimmed.slice(key.length, eq);
  const afterEq = trimmed.slice(eq + 1);
  const valueLead = afterEq.match(/^\s*/)?.[0] ?? "";
  const value = afterEq.slice(valueLead.length);

  return createElement(
    "span",
    { key: lineKey, className: "toml-line" },
    indent,
    tok("t-key", key, `${lineKey}-k`),
    afterKey,
    tok("t-punct", "=", `${lineKey}-eq`),
    valueLead,
    ...highlightTomlValue(value, `${lineKey}-v`),
  );
}

export function highlightToml(source: string): ReactNode[] {
  const lines = source.replace(/\n$/, "").split("\n");
  return lines.map((line, i) => highlightTomlLine(line, i));
}

function highlightJsonLine(line: string, lineKey: number): ReactNode {
  const out: ReactNode[] = [];
  let i = 0;
  let part = 0;

  while (i < line.length) {
    const ch = line[i]!;

    if (/\s/.test(ch)) {
      let j = i + 1;
      while (j < line.length && /\s/.test(line[j]!)) j++;
      out.push(line.slice(i, j));
      i = j;
      continue;
    }

    if (ch === '"') {
      let j = i + 1;
      while (j < line.length) {
        if (line[j] === "\\" && j + 1 < line.length) {
          j += 2;
          continue;
        }
        if (line[j] === '"') {
          j++;
          break;
        }
        j++;
      }
      const str = line.slice(i, j);
      let k = j;
      while (k < line.length && /\s/.test(line[k]!)) k++;
      if (line[k] === ":") {
        out.push(tok("t-key", str, `${lineKey}-k${part++}`));
      } else {
        out.push(...highlightString(str, `${lineKey}-s${part++}`));
      }
      i = j;
      continue;
    }

    if ("{}[],:".includes(ch)) {
      out.push(tok("t-punct", ch, `${lineKey}-p${part++}`));
      i++;
      continue;
    }

    if (/[0-9-]/.test(ch)) {
      let j = i + 1;
      while (j < line.length && /[0-9.]/.test(line[j]!)) j++;
      out.push(tok("t-num", line.slice(i, j), `${lineKey}-n${part++}`));
      i = j;
      continue;
    }

    if (/[A-Za-z_]/.test(ch)) {
      let j = i + 1;
      while (j < line.length && /[A-Za-z0-9_]/.test(line[j]!)) j++;
      const word = line.slice(i, j);
      if (word === "true" || word === "false" || word === "null") {
        out.push(tok("t-bool", word, `${lineKey}-b${part++}`));
      } else {
        out.push(word);
      }
      i = j;
      continue;
    }

    out.push(ch);
    i++;
  }

  return createElement(
    "span",
    { key: lineKey, className: "toml-line" },
    ...out,
  );
}

export function highlightJson(source: string): ReactNode[] {
  const lines = source.replace(/\n$/, "").split("\n");
  return lines.map((line, i) => highlightJsonLine(line, i));
}

function highlightShellLine(line: string, lineKey: number): ReactNode {
  const trimmed = line.trimStart();
  const indent = line.slice(0, line.length - trimmed.length);

  if (trimmed.startsWith("#")) {
    return createElement(
      "span",
      { key: lineKey, className: "toml-line" },
      indent,
      tok("t-comment", trimmed, `${lineKey}-c`),
    );
  }

  if (trimmed === "") {
    return createElement("span", {
      key: lineKey,
      className: "toml-line toml-line-empty",
    });
  }

  if (trimmed === "\\") {
    return createElement(
      "span",
      { key: lineKey, className: "toml-line" },
      tok("t-punct", "\\", `${lineKey}-cont`),
    );
  }

  const out: ReactNode[] = [indent];
  let i = 0;
  let part = 0;
  let first = true;

  while (i < trimmed.length) {
    const ch = trimmed[i]!;

    if (/\s/.test(ch)) {
      let j = i + 1;
      while (j < trimmed.length && /\s/.test(trimmed[j]!)) j++;
      out.push(trimmed.slice(i, j));
      i = j;
      continue;
    }

    if (ch === '"') {
      let j = i + 1;
      while (j < trimmed.length) {
        if (trimmed[j] === "\\" && j + 1 < trimmed.length) {
          j += 2;
          continue;
        }
        if (trimmed[j] === '"') {
          j++;
          break;
        }
        j++;
      }
      out.push(...highlightString(trimmed.slice(i, j), `${lineKey}-q${part++}`));
      i = j;
      first = false;
      continue;
    }

    if (ch === "\\" && i === trimmed.length - 1) {
      out.push(tok("t-punct", "\\", `${lineKey}-p${part++}`));
      i++;
      continue;
    }

    if (ch === "-" && trimmed[i + 1] === "-") {
      let j = i + 2;
      while (j < trimmed.length && !/\s/.test(trimmed[j]!)) j++;
      out.push(tok("t-key", trimmed.slice(i, j), `${lineKey}-f${part++}`));
      i = j;
      first = false;
      continue;
    }

    let j = i + 1;
    while (j < trimmed.length && !/\s/.test(trimmed[j]!)) j++;
    const word = trimmed.slice(i, j);
    if (first) {
      out.push(tok("t-header", word, `${lineKey}-cmd${part++}`));
      first = false;
    } else if (word.startsWith("$") || word.includes("=")) {
      out.push(tok("t-interp", word, `${lineKey}-v${part++}`));
    } else {
      out.push(tok("t-str", word, `${lineKey}-a${part++}`));
    }
    i = j;
  }

  return createElement(
    "span",
    { key: lineKey, className: "toml-line" },
    ...out,
  );
}

export function highlightShell(source: string): ReactNode[] {
  const lines = source.replace(/\n$/, "").split("\n");
  return lines.map((line, i) => highlightShellLine(line, i));
}

function highlightTsLine(line: string, lineKey: number): ReactNode {
  const trimmed = line.trimStart();
  const indent = line.slice(0, line.length - trimmed.length);

  if (
    trimmed.startsWith("//") ||
    trimmed.startsWith("*") ||
    trimmed.startsWith("/*")
  ) {
    return createElement(
      "span",
      { key: lineKey, className: "toml-line" },
      indent,
      tok("t-comment", trimmed, `${lineKey}-c`),
    );
  }

  if (trimmed === "") {
    return createElement("span", {
      key: lineKey,
      className: "toml-line toml-line-empty",
    });
  }

  const keywords = new Set([
    "import",
    "from",
    "const",
    "let",
    "await",
    "async",
    "new",
    "return",
    "export",
    "type",
    "function",
  ]);

  const out: ReactNode[] = [indent];
  let i = 0;
  let part = 0;

  while (i < trimmed.length) {
    const ch = trimmed[i]!;

    if (/\s/.test(ch)) {
      let j = i + 1;
      while (j < trimmed.length && /\s/.test(trimmed[j]!)) j++;
      out.push(trimmed.slice(i, j));
      i = j;
      continue;
    }

    if (ch === '"' || ch === "'" || ch === "`") {
      const quote = ch;
      let j = i + 1;
      while (j < trimmed.length) {
        if (trimmed[j] === "\\" && j + 1 < trimmed.length) {
          j += 2;
          continue;
        }
        if (trimmed[j] === quote) {
          j++;
          break;
        }
        j++;
      }
      out.push(...highlightString(trimmed.slice(i, j), `${lineKey}-q${part++}`));
      i = j;
      continue;
    }

    if ("{}[],();.:".includes(ch)) {
      out.push(tok("t-punct", ch, `${lineKey}-p${part++}`));
      i++;
      continue;
    }

    if (/[A-Za-z_$]/.test(ch)) {
      let j = i + 1;
      while (j < trimmed.length && /[A-Za-z0-9_$]/.test(trimmed[j]!)) j++;
      const word = trimmed.slice(i, j);
      if (keywords.has(word)) {
        out.push(tok("t-header", word, `${lineKey}-kw${part++}`));
      } else if (word === "true" || word === "false" || word === "null") {
        out.push(tok("t-bool", word, `${lineKey}-b${part++}`));
      } else {
        out.push(tok("t-key", word, `${lineKey}-id${part++}`));
      }
      i = j;
      continue;
    }

    out.push(ch);
    i++;
  }

  return createElement(
    "span",
    { key: lineKey, className: "toml-line" },
    ...out,
  );
}

export function highlightTs(source: string): ReactNode[] {
  const lines = source.replace(/\n$/, "").split("\n");
  return lines.map((line, i) => highlightTsLine(line, i));
}

export type CodeLang = "toml" | "json" | "shell" | "ts";

export function highlightCode(source: string, lang: CodeLang): ReactNode[] {
  switch (lang) {
    case "toml":
      return highlightToml(source);
    case "json":
      return highlightJson(source);
    case "shell":
      return highlightShell(source);
    case "ts":
      return highlightTs(source);
  }
}
