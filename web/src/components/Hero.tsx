import { CopyPrompt } from "@/components/CopyPrompt";
import { HeroDemo } from "@/components/HeroDemo";
import { ThemeToggle } from "@/components/theme-toggle";

type Props = {
  reduceMotion: boolean;
};

export function Hero({ reduceMotion }: Props) {
  return (
    <>
      <header
        className="hero"
        {...(reduceMotion ? { "data-static": true } : {})}
      >
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
              target="_blank"
              rel="noopener noreferrer"
            >
              GitHub
            </a>
          </div>
        </nav>

        <div className="hero-copy">
          <p className="brand">stackless</p>
          <h1>
            One toml.
            <br />
            One ephemeral stack.
          </h1>
          <p className="lede">
            For agents to run your stack end to end.
          </p>
          <div className="cta-row">
            <CopyPrompt />
          </div>
        </div>
      </header>

      <section className="hero-code-band" aria-label="stackless.toml example">
        <div className="hero-code-rule" aria-hidden="true" />
        <div className="hero-code-inner">
          <HeroDemo />
        </div>
      </section>
    </>
  );
}
