import { SparklesIcon, TerminalIcon } from "lucide-react";

import { ThemeToggle } from "@/components/theme-toggle";
import { Button } from "@/components/ui/button";
import { HeroDemo } from "@/components/HeroDemo";
import { useCopy } from "@/hooks/use-copy";
import type { Conductor } from "@/lib/conductor";
import { INSTALL_CMD, SKILLS_CMD } from "@/lib/copy";
import { cn } from "@/lib/utils";

type Props = {
  reduceMotion: boolean;
  conductor: Conductor;
};

export function Hero({ reduceMotion, conductor }: Props) {
  const { copied: installCopied, copy: copyInstall } = useCopy();
  const { copied: skillsCopied, copy: copySkills } = useCopy();

  return (
    <header className="hero" {...(reduceMotion ? { "data-static": true } : {})}>
      <div className="hero-atmosphere" aria-hidden="true">
        <div className="hero-grid" />
        <div className="hero-wash" />
      </div>

      <nav className="topbar">
        <span className="topbar-spacer" aria-hidden="true" />
        <div className="topbar-actions">
          <ThemeToggle />
          <a
            className="topbar-link"
            href="https://github.com/snowmead/stackless"
            rel="noopener noreferrer"
          >
            GitHub
          </a>
        </div>
      </nav>

      <div className="hero-split">
        <div className="hero-copy">
          <p className="brand">stackless</p>
          <h1>
            One toml.
            <br />
            One ephemeral stack.
          </h1>
          <p className="lede">
            Ephemeral e2e stacks: your services, Stripe Projects integrations,{" "}
            <code>up</code> / <code>verify</code> / <code>down</code> — or lease
            expiry.
          </p>
          <div className="cta-row">
            <Button
              type="button"
              size="lg"
              variant="outline"
              className={cn(
                "hero-cmd h-auto rounded-sm px-[1.1rem] py-[0.72rem] text-[0.95rem]",
                installCopied && "is-copied",
              )}
              aria-label={
                installCopied ? "Copied install command" : "Copy install command"
              }
              onClick={() => void copyInstall(INSTALL_CMD)}
            >
              <TerminalIcon className="hero-cmd-icon" aria-hidden="true" />
              <span>{installCopied ? "Copied" : "Install CLI"}</span>
            </Button>
            <Button
              type="button"
              size="lg"
              variant="outline"
              className={cn(
                "hero-cmd h-auto rounded-sm px-[1.1rem] py-[0.72rem] text-[0.95rem]",
                skillsCopied && "is-copied",
              )}
              aria-label={
                skillsCopied ? "Copied skill command" : "Copy skill command"
              }
              onClick={() => void copySkills(SKILLS_CMD)}
            >
              <SparklesIcon className="hero-cmd-icon" aria-hidden="true" />
              <span>{skillsCopied ? "Copied" : "Add skill"}</span>
            </Button>
          </div>
        </div>

        <HeroDemo reduceMotion={reduceMotion} conductor={conductor} />
      </div>
    </header>
  );
}
