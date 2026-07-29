import { ThemeToggle } from "@/components/theme-toggle";
import { Button } from "@/components/ui/button";
import { HeroDemo } from "@/components/HeroDemo";
import { useCopy } from "@/hooks/use-copy";
import type { Conductor } from "@/lib/conductor";
import { INSTALL_CMD } from "@/lib/copy";
import { cn } from "@/lib/utils";

type Props = {
  reduceMotion: boolean;
  conductor: Conductor;
};

export function Hero({ reduceMotion, conductor }: Props) {
  const { copied, copy } = useCopy();

  return (
    <header className="hero">
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
            One disposable stack.
          </h1>
          <p className="lede">
            Disposable e2e stacks: your services, Stripe Projects integrations,{" "}
            <code>up</code> / <code>verify</code> / <code>down</code>.
          </p>
          <div className="cta-row">
            <Button
              type="button"
              size="lg"
              className={cn(
                "h-auto rounded-sm px-[1.1rem] py-[0.72rem] text-[0.95rem]",
                copied &&
                  "bg-[var(--signal)] text-[var(--void)] hover:bg-[var(--signal)]",
              )}
              onClick={() => void copy(INSTALL_CMD)}
            >
              {copied ? "Copied" : "Copy install"}
            </Button>
            <Button
              asChild
              variant="outline"
              size="lg"
              className="h-auto rounded-sm bg-transparent px-[1.1rem] py-[0.72rem] text-[0.95rem]"
            >
              <a
                href="https://github.com/snowmead/stackless"
                rel="noopener noreferrer"
              >
                View on GitHub
              </a>
            </Button>
          </div>
        </div>

        <HeroDemo reduceMotion={reduceMotion} conductor={conductor} />
      </div>
    </header>
  );
}
