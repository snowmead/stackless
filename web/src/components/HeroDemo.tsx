import { FileIcon, GlobeIcon, TerminalIcon } from "lucide-react";

export function HeroDemo() {
  return (
    <div className="hero-code">
      <div className="hero-code-chrome">
        <div className="hero-code-file">
          <FileIcon className="hero-code-icon" aria-hidden="true" />
          <span>stackless.toml</span>
        </div>
        <div className="hero-code-modes">
          <div
            className="hero-code-mode hero-code-mode-up"
            title="Bring a stack up from the CLI"
          >
            <TerminalIcon className="hero-code-icon" aria-hidden="true" />
            <span>stackless up --name demo</span>
          </div>
          <div
            className="hero-code-mode hero-code-mode-down"
            title="Tear a stack down from the CLI"
          >
            <GlobeIcon className="hero-code-icon" aria-hidden="true" />
            <span>stackless down demo</span>
          </div>
        </div>
      </div>
      <pre className="hero-code-body" tabIndex={0}>
        <code>
          <span className="toml-line">
            <span className="t-header">[stack]</span>
          </span>
          <span className="toml-line">
            <span className="t-key">name</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">&quot;demo&quot;</span>
          </span>
          <span className="toml-line toml-line-empty" />
          <span className="toml-line">
            <span className="t-header">[integrations.clerk]</span>
          </span>
          <span className="toml-line">
            <span className="t-key">provider</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">&quot;clerk&quot;</span>
          </span>
          <span className="toml-line">
            <span className="t-key">app_name</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">
              &quot;
              <span className="t-interp">${"{"}stack.name{"}"}</span>-
              <span className="t-interp">${"{"}instance.name{"}"}</span>
              &quot;
            </span>
          </span>
          <span className="toml-line toml-line-empty" />
          <span className="toml-line">
            <span className="t-header">[integrations.neon]</span>
          </span>
          <span className="toml-line">
            <span className="t-key">provider</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">&quot;neon&quot;</span>
          </span>
          <span className="toml-line toml-line-empty" />
          <span className="toml-line">
            <span className="t-header">[services.web]</span>
          </span>
          <span className="toml-line">
            <span className="t-key">source</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-punct">{"{"}</span>{" "}
            <span className="t-key">repo</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">
              &quot;https://github.com/you/app&quot;
            </span>
            <span className="t-punct">,</span>{" "}
            <span className="t-key">ref</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">&quot;main&quot;</span>{" "}
            <span className="t-punct">{"}"}</span>
          </span>
          <span className="toml-line">
            <span className="t-key">root_origin</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-bool">true</span>
          </span>
          <span className="toml-line">
            <span className="t-key">health</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-punct">{"{"}</span>{" "}
            <span className="t-key">path</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">&quot;/&quot;</span>
            <span className="t-punct">,</span>{" "}
            <span className="t-key">contains</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">&quot;ok&quot;</span>{" "}
            <span className="t-punct">{"}"}</span>
          </span>
          <span className="toml-line">
            <span className="t-key">env</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-punct">{"{"}</span>{" "}
            <span className="t-key">CLERK_SECRET_KEY</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">
              &quot;
              <span className="t-interp">
                ${"{"}integrations.clerk.secret_key{"}"}
              </span>
              &quot;
            </span>
            <span className="t-punct">,</span>{" "}
            <span className="t-key">DATABASE_URL</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">
              &quot;
              <span className="t-interp">
                ${"{"}integrations.neon.database_url{"}"}
              </span>
              &quot;
            </span>{" "}
            <span className="t-punct">{"}"}</span>
          </span>
          <span className="toml-line toml-line-empty" />
          <span className="toml-line">
            <span className="t-header">{"  "}[services.web.vercel]</span>
          </span>
          <span className="toml-line">
            <span className="t-key">{"  "}framework</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">&quot;vite&quot;</span>
          </span>
          <span className="toml-line">
            <span className="t-key">{"  "}build</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">&quot;npm run build&quot;</span>
          </span>
          <span className="toml-line toml-line-empty" />
          <span className="toml-line">
            <span className="t-header">[services.db]</span>
          </span>
          <span className="toml-line">
            <span className="t-key">source</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-punct">{"{"}</span>{" "}
            <span className="t-key">repo</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">
              &quot;https://github.com/you/app&quot;
            </span>
            <span className="t-punct">,</span>{" "}
            <span className="t-key">ref</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">&quot;main&quot;</span>{" "}
            <span className="t-punct">{"}"}</span>
          </span>
          <span className="toml-line">
            <span className="t-key">health</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-punct">{"{"}</span>{" "}
            <span className="t-key">path</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">&quot;/health&quot;</span>
            <span className="t-punct">,</span>{" "}
            <span className="t-key">contains</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">&quot;ready&quot;</span>{" "}
            <span className="t-punct">{"}"}</span>
          </span>
          <span className="toml-line toml-line-empty" />
          <span className="toml-line">
            <span className="t-header">{"  "}[services.db.local]</span>
          </span>
          <span className="toml-line">
            <span className="t-key">{"  "}run</span>{" "}
            <span className="t-punct">=</span>{" "}
            <span className="t-str">
              &quot;docker run --rm -p $PORT:5432 postgres:16&quot;
            </span>
          </span>
        </code>
      </pre>
    </div>
  );
}
