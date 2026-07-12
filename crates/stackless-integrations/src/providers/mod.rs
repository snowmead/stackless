pub mod agentmail;
pub mod agentphone;
pub mod algolia;
pub mod amplitude;
pub mod auth0;
pub mod base44_projects;
pub mod blaxel;
pub mod browserbase;
pub mod chroma;
pub mod clerk;
pub mod clickhouse;
pub mod cloudflare;
pub mod daytona;
pub mod e2b;
pub mod elevenlabs;
pub mod exa;
pub mod firecrawl;
pub mod flyio;
pub mod gitlab;
pub mod heygen;
pub mod huggingface;
pub mod inngest;
pub mod kernel;
pub mod laravel_cloud;
pub mod metronome;
pub mod neon;
pub mod openrouter;
pub mod parallel;
pub mod planetscale;
pub mod postalform;
pub mod posthog;
pub mod prisma;
pub mod privy;
pub mod runloop;
pub mod supermemory;
pub mod turso;
pub mod upstash;
pub mod workos;

#[cfg(test)]
mod tests {
    use stackless_provider_sdk::CatalogResource;
    use stackless_provider_sdk::Hostable;

    use crate::providers::{
        agentmail, agentphone, algolia, amplitude, auth0, base44_projects, blaxel, browserbase,
        chroma, clickhouse, cloudflare, daytona, e2b, elevenlabs, exa, firecrawl, flyio, gitlab,
        heygen, huggingface, inngest, kernel, laravel_cloud, metronome, neon, openrouter, parallel,
        planetscale, postalform, posthog, prisma, privy, runloop, supermemory, turso, upstash,
        workos,
    };

    fn assert_outputs_match<T: CatalogResource>() {
        let fields: Vec<&str> = T::OUTPUT_FIELDS.iter().map(|(_, name, _)| *name).collect();
        let outputs: Vec<&str> = <T as Hostable>::OUTPUTS.to_vec();
        assert_eq!(
            outputs,
            fields,
            "{}: Hostable::OUTPUTS drifted from CatalogResource::OUTPUT_FIELDS names",
            T::PROVIDER
        );
    }

    #[test]
    fn catalog_outputs_match_output_fields() {
        assert_outputs_match::<cloudflare::r2::CloudflareR2>();
        assert_outputs_match::<cloudflare::kv::CloudflareKv>();
        assert_outputs_match::<cloudflare::d1::CloudflareD1>();
        assert_outputs_match::<cloudflare::queues::CloudflareQueues>();
        assert_outputs_match::<cloudflare::hyperdrive::CloudflareHyperdrive>();
        assert_outputs_match::<cloudflare::workers::CloudflareWorkers>();
        assert_outputs_match::<cloudflare::workers_ai::CloudflareWorkersAi>();
        assert_outputs_match::<cloudflare::browser_run::CloudflareBrowserRun>();
        assert_outputs_match::<agentmail::api::AgentMailApi>();
        assert_outputs_match::<agentphone::number::AgentPhoneNumber>();
        assert_outputs_match::<algolia::application::AlgoliaApplication>();
        assert_outputs_match::<amplitude::analytics::AmplitudeAnalytics>();
        assert_outputs_match::<auth0::client::Auth0Client>();
        assert_outputs_match::<base44_projects::app::Base44ProjectsApp>();
        assert_outputs_match::<blaxel::agent_drive::BlaxelAgentDrive>();
        assert_outputs_match::<blaxel::sandbox::BlaxelSandbox>();
        assert_outputs_match::<browserbase::project::BrowserbaseProject>();
        assert_outputs_match::<chroma::database::ChromaDatabase>();
        assert_outputs_match::<clickhouse::cluster::ClickHouseClickhouse>();
        assert_outputs_match::<clickhouse::postgres::ClickHousePostgres>();
        assert_outputs_match::<daytona::sandbox::DaytonaSandbox>();
        assert_outputs_match::<e2b::sandbox::E2BSandbox>();
        assert_outputs_match::<elevenlabs::tts::ElevenLabsTts>();
        assert_outputs_match::<exa::api::ExaApi>();
        assert_outputs_match::<firecrawl::api::FirecrawlApi>();
        assert_outputs_match::<flyio::mpg::FlyioMpg>();
        assert_outputs_match::<flyio::sprite::FlyioSprite>();
        assert_outputs_match::<gitlab::project::GitLabProject>();
        assert_outputs_match::<heygen::api::HeyGenApi>();
        assert_outputs_match::<huggingface::bucket::HuggingFaceBucket>();
        assert_outputs_match::<huggingface::platform::HuggingFacePlatform>();
        assert_outputs_match::<inngest::app::InngestApp>();
        assert_outputs_match::<kernel::project::KERNELProject>();
        assert_outputs_match::<laravel_cloud::application::LaravelCloudApplication>();
        assert_outputs_match::<laravel_cloud::mysql::LaravelCloudMysql>();
        assert_outputs_match::<laravel_cloud::valkey::LaravelCloudValkey>();
        assert_outputs_match::<metronome::sandbox::MetronomeSandbox>();
        assert_outputs_match::<neon::postgres::NeonPostgres>();
        assert_outputs_match::<openrouter::api::OpenRouterApi>();
        assert_outputs_match::<parallel::api::ParallelApi>();
        assert_outputs_match::<planetscale::mysql::PlanetScaleMysql>();
        assert_outputs_match::<planetscale::postgresql::PlanetScalePostgresql>();
        assert_outputs_match::<postalform::mail::PostalFormMail>();
        assert_outputs_match::<posthog::analytics::PostHogAnalytics>();
        assert_outputs_match::<prisma::database::PrismaDatabase>();
        assert_outputs_match::<privy::app::PrivyApp>();
        assert_outputs_match::<runloop::sandbox::RunloopSandbox>();
        assert_outputs_match::<supermemory::memory::SupermemoryMemory>();
        assert_outputs_match::<turso::database::TursoDatabase>();
        assert_outputs_match::<upstash::qstash::UpstashQstash>();
        assert_outputs_match::<upstash::redis::UpstashRedis>();
        assert_outputs_match::<upstash::search::UpstashSearch>();
        assert_outputs_match::<upstash::vector::UpstashVector>();
        assert_outputs_match::<workos::auth::WorkOSAuth>();
    }
}
