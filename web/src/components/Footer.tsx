import { Separator } from "@/components/ui/separator";

export function Footer() {
  return (
    <footer className="footer">
      <div className="footer-row">
        <p className="footer-brand">stackless</p>
        <p className="footer-meta">MIT · v0.1.5</p>
      </div>
      <Separator className="mb-4" />
      <nav className="footer-nav" aria-label="Documentation">
        <a
          href="https://github.com/snowmead/stackless"
          rel="noopener noreferrer"
        >
          GitHub
        </a>
        <a
          href="https://github.com/snowmead/stackless/blob/main/docs/SCHEMA.md"
          rel="noopener noreferrer"
        >
          SCHEMA
        </a>
        <a
          href="https://github.com/snowmead/stackless/blob/main/docs/AGENT-FLEETS.md"
          rel="noopener noreferrer"
        >
          fleets
        </a>
        <a
          href="https://github.com/snowmead/stackless/blob/main/.cursor/skills/stackless/SKILL.md"
          rel="noopener noreferrer"
        >
          skill
        </a>
        <a href="/llms.txt">llms.txt</a>
      </nav>
    </footer>
  );
}
