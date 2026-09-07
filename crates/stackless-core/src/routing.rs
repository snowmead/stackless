//! Dispatch by hosting placement. Resource ownership remains in the journal.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::capabilities::Capabilities;
use crate::def::{DefError, Namespace, StackDef, placement::placement_key};
use crate::engine::{Step, plan::ExecutionPlan};
use crate::state::{Checkpoint, ResourceRecord, Store};
use crate::substrate::{
    InstanceContext, NamespacePurpose, Observation, ServiceLog, SpendInfo, StepContext,
    StepResource, Substrate, SubstrateFault,
};

pub struct RoutedSubstrate {
    default: String,
    definition: StackDef,
    store: Option<Store>,
    providers: BTreeMap<String, Box<dyn Substrate>>,
}

impl std::fmt::Debug for RoutedSubstrate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutedSubstrate")
            .field("default", &self.default)
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl RoutedSubstrate {
    /// Include adapters from both the desired definition and the retained journal.
    /// Construction performs no provider operations.
    pub fn new(
        store: Option<Store>,
        default: String,
        mut definition: StackDef,
        recorded: BTreeMap<String, String>,
        providers: BTreeMap<String, Box<dyn Substrate>>,
    ) -> Result<Self, SubstrateFault> {
        let required: BTreeSet<_> = definition
            .placements(&default)
            .into_values()
            .chain(recorded.values().cloned())
            .chain(std::iter::once(default.clone()))
            .collect();
        for name in required {
            if providers
                .get(&name)
                .is_none_or(|provider| provider.name() != name)
            {
                return Err(crate::capabilities::unsupported_feature(
                    &name,
                    "placement",
                    "an unregistered hosting adapter",
                ));
            }
        }
        for workload in definition.services.values_mut() {
            workload.on.get_or_insert_with(|| default.clone());
        }
        for resource in definition.integrations.values_mut() {
            resource.on.get_or_insert_with(|| default.clone());
        }
        Ok(Self {
            default,
            definition,
            store,
            providers,
        })
    }

    fn provider(&self, name: &str) -> &dyn Substrate {
        // The constructor checks the complete routing domain.
        self.providers[name].as_ref()
    }

    fn desired(&self, step: &Step) -> &dyn Substrate {
        self.provider(self.definition.step_provider(&self.default, step))
    }

    fn recorded_provider(&self, owner: &str, step: &str) -> Result<&dyn Substrate, SubstrateFault> {
        let placements = self
            .store
            .as_ref()
            .ok_or_else(|| {
                crate::capabilities::unsupported_feature(
                    &self.default,
                    step,
                    "resource observation without a state journal",
                )
            })?
            .placements(owner)
            .map_err(|error| SubstrateFault::from_fault(&error))?;
        let name = placement_key(step)
            .and_then(|key| placements.get(&key))
            .map(String::as_str)
            .unwrap_or(&self.default);
        self.providers.get(name).map(Box::as_ref).ok_or_else(|| {
            crate::capabilities::unsupported_feature(
                name,
                step,
                "an unregistered recorded hosting adapter",
            )
        })
    }

    fn origins(
        &self,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        purpose: NamespacePurpose,
    ) -> BTreeMap<String, String> {
        let raw = InstanceContext {
            routed_origins: None,
            ..*instance
        };
        let mut origins = BTreeMap::new();
        for (name, provider) in &self.providers {
            let namespace =
                provider.build_namespace(&self.definition, &raw, prior, secrets, purpose);
            for (service, origin) in namespace.service_origins {
                if self
                    .definition
                    .services
                    .get(&service)
                    .is_some_and(|spec| spec.on.as_deref() == Some(name))
                    && !origin.is_empty()
                {
                    origins.insert(service, origin);
                }
            }
        }
        origins
    }
}

#[async_trait::async_trait]
impl Substrate for RoutedSubstrate {
    fn name(&self) -> &str {
        &self.default
    }

    fn capabilities(&self) -> Capabilities {
        self.provider(&self.default).capabilities()
    }

    fn can_manage_resource(&self, resource: &ResourceRecord) -> bool {
        self.recorded_provider(&resource.owner_id, &resource.step_id)
            .is_ok_and(|provider| provider.can_manage_resource(resource))
    }

    fn execution_plan(&self, def: &StackDef) -> Result<ExecutionPlan, DefError> {
        def.execution_plan_with_placement(&self.default, |name| {
            self.providers
                .get(name)
                .is_some_and(|provider| provider.capabilities().early_origins)
        })
    }

    fn validate(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        let mut requested = def.clone();
        for workload in requested.services.values_mut() {
            workload.on.get_or_insert_with(|| self.default.clone());
        }
        for resource in requested.integrations.values_mut() {
            resource.on.get_or_insert_with(|| self.default.clone());
        }
        if crate::engine::revision::digest(&requested)?
            != crate::engine::revision::digest(&self.definition)?
        {
            return Err(SubstrateFault::from_fault(
                &crate::state::StateError::ResourceInvariant {
                    detail: "routing adapter and requested definition differ".into(),
                },
            ));
        }
        let active: BTreeSet<_> = self
            .definition
            .placements(&self.default)
            .into_values()
            .collect();
        for name in active {
            let provider = self.provider(&name);
            provider.capabilities().validate(&name, &self.definition)?;
            provider.validate_definition(&self.definition)?;
        }
        self.execution_plan(&self.definition)
            .map_err(|error| SubstrateFault::from_fault(&error))?;
        Ok(())
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        self.validate(def)
    }

    fn supports_source_override(&self) -> bool {
        self.provider(&self.default).supports_source_override()
    }

    fn supports_source_override_for(&self, def: &StackDef, service: &str) -> bool {
        let name = def
            .services
            .get(service)
            .and_then(|spec| spec.on.as_deref())
            .unwrap_or(&self.default);
        self.provider(name)
            .supports_source_override_for(def, service)
    }

    fn default_lease(&self) -> Duration {
        self.providers
            .values()
            .map(|provider| provider.default_lease())
            .min()
            .unwrap_or_default()
    }

    fn service_origin(
        &self,
        _def: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
    ) -> String {
        self.origins(
            instance,
            instance.checkpoints,
            &BTreeMap::new(),
            NamespacePurpose::ServiceEnv,
        )
        .remove(service)
        .unwrap_or_default()
    }

    fn build_namespace(
        &self,
        _def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        purpose: NamespacePurpose,
    ) -> Namespace {
        let origins = self.origins(instance, prior, secrets, purpose);
        let routed = InstanceContext {
            routed_origins: Some(&origins),
            ..*instance
        };
        let mut namespace = self.provider(&self.default).build_namespace(
            &self.definition,
            &routed,
            prior,
            secrets,
            purpose,
        );
        routed.bind_namespace(&mut namespace, &self.definition);
        namespace
    }

    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        let origins = self.origins(
            ctx.instance,
            ctx.prior,
            &BTreeMap::new(),
            NamespacePurpose::ServiceEnv,
        );
        let instance = InstanceContext {
            routed_origins: Some(&origins),
            ..*ctx.instance
        };
        self.desired(ctx.step)
            .execute(StepContext {
                instance: &instance,
                def: &self.definition,
                ..ctx
            })
            .await
    }

    fn step_revision(&self, ctx: &StepContext<'_>) -> Result<String, SubstrateFault> {
        let origins = self.origins(
            ctx.instance,
            ctx.prior,
            &BTreeMap::new(),
            NamespacePurpose::ServiceEnv,
        );
        let instance = InstanceContext {
            routed_origins: Some(&origins),
            ..*ctx.instance
        };
        self.desired(ctx.step).step_revision(&StepContext {
            operation_id: ctx.operation_id,
            store: ctx.store,
            instance: &instance,
            def: &self.definition,
            step: ctx.step,
            source_overrides: ctx.source_overrides,
            dirty: ctx.dirty,
            prior: ctx.prior,
            parent_resources: ctx.parent_resources,
            cancelled: ctx.cancelled.clone(),
        })
    }

    fn refresh_each_operation(&self, step: &Step) -> bool {
        self.desired(step).refresh_each_operation(step)
    }

    async fn reconcile(
        &self,
        ctx: StepContext<'_>,
        previous: &Checkpoint,
    ) -> Result<StepResource, SubstrateFault> {
        let origins = self.origins(
            ctx.instance,
            ctx.prior,
            &BTreeMap::new(),
            NamespacePurpose::ServiceEnv,
        );
        let instance = InstanceContext {
            routed_origins: Some(&origins),
            ..*ctx.instance
        };
        self.desired(ctx.step)
            .reconcile(
                StepContext {
                    instance: &instance,
                    def: &self.definition,
                    ..ctx
                },
                previous,
            )
            .await
    }

    async fn observe(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<Observation, SubstrateFault> {
        self.recorded_provider(instance.id, &checkpoint.step_id)?
            .observe(instance, checkpoint)
            .await
    }

    async fn destroy(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        self.recorded_provider(instance.id, &checkpoint.step_id)?
            .destroy(instance, checkpoint)
            .await
    }

    async fn destroy_record(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
        resource: &ResourceRecord,
    ) -> Result<(), SubstrateFault> {
        self.recorded_provider(instance.id, &resource.step_id)?
            .destroy_record(store, instance, resource)
            .await
    }

    async fn observe_record(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
        resource: &ResourceRecord,
    ) -> Result<Observation, SubstrateFault> {
        self.recorded_provider(instance.id, &resource.step_id)?
            .observe_record(store, instance, resource)
            .await
    }

    async fn restore_routes(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        for provider in self.providers.values() {
            provider.restore_routes(store, instance).await?;
        }
        Ok(())
    }

    async fn finalize_teardown(
        &self,
        instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        for provider in self.providers.values() {
            provider.finalize_teardown(instance).await?;
        }
        Ok(())
    }

    async fn spend(&self) -> Option<SpendInfo> {
        let mut entries = Vec::new();
        for provider in self.providers.values() {
            if let Some(info) = provider.spend().await {
                entries.push(info);
            }
        }
        match entries.len() {
            0 => None,
            1 => entries.pop(),
            _ => Some(SpendInfo {
                provider: "mixed".into(),
                cap_usd: entries.iter().map(|entry| entry.cap_usd).sum(),
                summary: entries
                    .iter()
                    .map(|entry| entry.summary.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
                data: Some(serde_json::json!({ "providers": entries })),
            }),
        }
    }

    async fn fetch_logs(
        &self,
        store: &Store,
        _def: &StackDef,
        instance: &InstanceContext<'_>,
        services: &[String],
        tail: usize,
    ) -> Result<Option<Vec<ServiceLog>>, SubstrateFault> {
        let mut groups: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for service in services {
            let provider = self.recorded_provider(instance.id, &format!("start:{service}"))?;
            groups
                .entry(provider.name())
                .or_default()
                .push(service.clone());
        }
        let mut result = None;
        let mut unavailable = Vec::new();
        for (name, services) in groups {
            if let Some(mut logs) = self
                .provider(name)
                .fetch_logs(store, &self.definition, instance, &services, tail)
                .await?
            {
                result.get_or_insert_with(Vec::new).append(&mut logs);
            } else {
                unavailable.extend(services.into_iter().map(|service| ServiceLog {
                    service,
                    source: "unavailable",
                    log_path: None,
                    lines: Vec::new(),
                }));
            }
        }
        if let Some(logs) = &mut result {
            logs.extend(unavailable);
        }
        Ok(result)
    }
}
