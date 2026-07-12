pub mod agentmail;
pub mod agentphone;
pub mod clerk;
pub mod cloudflare;

#[cfg(test)]
mod tests {
    use stackless_provider_sdk::CatalogResource;
    use stackless_provider_sdk::Hostable;

    use crate::providers::{agentmail, agentphone, cloudflare};

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
    }
}
