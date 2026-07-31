#!/usr/bin/env bash
# Build and open wave + substrate PRs from the full catalog branch tip.
set -euo pipefail
cd "$(dirname "$0")/.."
TIP=$(git rev-parse feat/stripe-catalog-providers)
MAIN=origin/main

WAVES=(
  "1:neon,supabase,turso,upstash,auth0,workos,privy,prisma"
  "2:planetscale,clickhouse,chroma,sentry,posthog,amplitude,mixpanel,algolia"
  "3:openrouter,exa,firecrawl,parallel,elevenlabs,heygen,huggingface,inngest"
  "4:e2b,daytona,browserbase,blaxel,runloop,kernel,agentmail,agentphone"
  "5:railway,gitlab,laravel_cloud,wordpress_com,base44_projects,wix,postalform,metronome,supermemory"
  "6:render_db,flyio"
)

build_wave() {
  local num="$1"
  local fams="$2"
  local branch="feat/integrations-wave-${num}"
  echo "=== Building $branch ($fams) ==="
  git checkout -B "$branch" "$MAIN"
  # infra from tip
  git checkout "$TIP" -- \
    crates/stackless-provider-sdk/src/resource.rs \
    docs/ADDING-A-PROVIDER.md \
    scripts/generate_catalog_integrations.py || true
  IFS=',' read -ra FAR <<< "$fams"
  for fam in "${FAR[@]}"; do
    git checkout "$TIP" -- "crates/stackless-integrations/src/providers/$fam"
  done
  # rebuild registry/mod from tip filtered
  python3 - <<PY
from pathlib import Path
import re
tip_reg = Path("/tmp/tip-registry.rs")
# use current tip files via git show
import subprocess
def show(path):
    return subprocess.check_output(["git","show",f"$TIP:{path}"], text=True)

reg = show("crates/stackless-integrations/src/registry.rs")
mod = show("crates/stackless-integrations/src/providers/mod.rs")
# start from main versions
base_reg = subprocess.check_output(["git","show",f"$MAIN:crates/stackless-integrations/src/registry.rs"], text=True)
base_mod = subprocess.check_output(["git","show",f"$MAIN:crates/stackless-integrations/src/providers/mod.rs"], text=True)

fams = """$fams""".split(",")
rows = []
for fam in fams:
    rows += re.findall(rf"    \({fam}::[\w:]+, \w+\),", reg)

m = re.search(r"register_providers! \{([^}]*)\}", base_reg, re.S)
body = m.group(1).rstrip() + "\n" + "\n".join(rows)
new_reg = re.sub(r"register_providers! \{[^}]*\}", "register_providers! {" + body + "\n}", base_reg, count=1, flags=re.S)
Path("crates/stackless-integrations/src/registry.rs").write_text(new_reg)

# mod.rs
lines = ["pub mod clerk;", "pub mod cloudflare;"]
for fam in sorted(fams):
    lines.append(f"pub mod {fam};")
# keep tests from tip but filter asserts
asserts = []
for fam in fams:
    asserts += re.findall(rf"        assert_outputs_match::<{fam}::[^>]+>\(\);", mod)
use_fams = ", ".join(sorted(fams))
test_block = f'''
#[cfg(test)]
mod tests {{
    use stackless_provider_sdk::CatalogResource;
    use stackless_provider_sdk::Hostable;

    use crate::providers::{{cloudflare, {use_fams}}};

    fn assert_outputs_match<T: CatalogResource>() {{
        let fields: Vec<&str> = T::OUTPUT_FIELDS.iter().map(|(_, name, _)| *name).collect();
        let outputs: Vec<&str> = <T as Hostable>::OUTPUTS.to_vec();
        assert_eq!(
            outputs,
            fields,
            "{{}}: Hostable::OUTPUTS drifted from CatalogResource::OUTPUT_FIELDS names",
            T::PROVIDER
        );
    }}

    #[test]
    fn catalog_outputs_match_output_fields() {{
        assert_outputs_match::<cloudflare::r2::CloudflareR2>();
        assert_outputs_match::<cloudflare::kv::CloudflareKv>();
        assert_outputs_match::<cloudflare::d1::CloudflareD1>();
        assert_outputs_match::<cloudflare::queues::CloudflareQueues>();
        assert_outputs_match::<cloudflare::hyperdrive::CloudflareHyperdrive>();
        assert_outputs_match::<cloudflare::workers::CloudflareWorkers>();
        assert_outputs_match::<cloudflare::workers_ai::CloudflareWorkersAi>();
        assert_outputs_match::<cloudflare::browser_run::CloudflareBrowserRun>();
{chr(10).join(asserts)}
    }}
}}
'''
Path("crates/stackless-integrations/src/providers/mod.rs").write_text("\n".join(lines) + "\n" + test_block)
print("wrote registry+mod for", fams)
PY
  # docs checklist only on wave 1 + copy SCHEMA/README partial on last wave
  if [[ "$num" == "1" ]]; then
    git add docs/ADDING-A-PROVIDER.md scripts/generate_catalog_integrations.py crates/stackless-provider-sdk/src/resource.rs
  fi
  git add crates/stackless-integrations
  git commit -m "feat(integrations): wave ${num} Stripe catalog providers (${fams})" || true
}

# fetch tip registry into nothing — build_wave uses git show

for entry in "${WAVES[@]}"; do
  num="${entry%%:*}"
  fams="${entry#*:}"
  build_wave "$num" "$fams"
done

echo DONE_WAVES
