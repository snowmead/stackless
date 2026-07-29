import { CheckIcon, CopyIcon } from "lucide-react";

import { CodeBlock } from "@/components/CodeBlock";
import { Button } from "@/components/ui/button";
import { useCopy } from "@/hooks/use-copy";
import { INSTALL_CMD } from "@/lib/copy";
import { cn } from "@/lib/utils";

const LIFECYCLE_SHELL = `stackless check stackless.toml --on local --json
stackless up --name demo --on local --json
stackless verify demo --json
stackless down demo --json`;

export function Install() {
  const { copied, copy } = useCopy();

  return (
    <section className="section install" id="install">
      <p className="section-label">Install</p>
      <h2>Install once. Hand the rest to an agent.</h2>
      <p>
        Latest release installer. Expect <code>stackless 0.1.7</code> or newer.
      </p>
      <div className="code-with-copy">
        <CodeBlock
          code={INSTALL_CMD}
          lang="shell"
          className="install-cmd"
          id="install-cmd"
        />
        <Button
          type="button"
          variant="outline"
          size="icon-sm"
          className={cn(
            "copy-icon absolute top-[0.65rem] right-[0.65rem] rounded-sm",
            copied &&
              "is-copied text-[var(--signal)] border-[var(--signal-dim)]",
          )}
          aria-label={copied ? "Copied" : "Copy install command"}
          onClick={() => void copy(INSTALL_CMD)}
        >
          {copied ? (
            <CheckIcon data-icon="inline-start" />
          ) : (
            <CopyIcon data-icon="inline-start" />
          )}
          <span className="visually-hidden">
            {copied ? "Copied" : "Copy"}
          </span>
        </Button>
      </div>
      <CodeBlock code={LIFECYCLE_SHELL} lang="shell" />
    </section>
  );
}
