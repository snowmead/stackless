import { FileIcon, TerminalIcon } from "lucide-react";

import { CodeBlock } from "@/components/CodeBlock";
import type { CodeLang } from "@/lib/highlight";
import { cn } from "@/lib/utils";

type Props = {
  code: string;
  lang: CodeLang;
  label: string;
  icon?: "file" | "terminal";
  className?: string;
};

export function CodePanel({
  code,
  lang,
  label,
  icon = "file",
  className,
}: Props) {
  const Icon = icon === "terminal" ? TerminalIcon : FileIcon;

  return (
    <div className={cn("hero-code code-panel", className)}>
      <div className="hero-code-chrome">
        <div className="hero-code-file">
          <Icon className="hero-code-icon" aria-hidden="true" />
          <span>{label}</span>
        </div>
      </div>
      <CodeBlock code={code} lang={lang} className="hero-code-body" />
    </div>
  );
}
