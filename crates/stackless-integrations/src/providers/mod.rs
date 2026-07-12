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

#[cfg(test)]
mod tests {
    use stackless_provider_sdk::CatalogResource;
    use stackless_provider_sdk::Hostable;

    use crate::providers::{
        agentmail, agentphone, algolia, amplitude, auth0, base44_projects, blaxel, browserbase,
        chroma, clickhouse, cloudflare, daytona,
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
    }
}
