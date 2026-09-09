//! Controller-owned Stripe resources share the provider's teardown journal.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use stackless_core::def::{Namespace, StackDef};
use stackless_core::state::{Checkpoint, ResourcePhase, ResourceRecord, Store};
use stackless_core::substrate::{
    InstanceContext, NamespacePurpose, Observation, ServiceLog, SpendInfo, StepContext,
    StepResource, Substrate, SubstrateFault,
};
use stackless_stripe_projects::{StripeProjects, TokioRunner, project};

use super::runtime::{ENVIRONMENT_KIND, RuntimeContext};

pub(super) struct ManagedSubstrate {
    inner: Box<dyn Substrate>,
    stripe_dir: PathBuf,
    project_id: Option<String>,
    state_root: PathBuf,
}

impl ManagedSubstrate {
    pub fn new(inner: Box<dyn Substrate>, runtime: &RuntimeContext) -> Self {
        Self {
            inner,
            stripe_dir: runtime.dir.clone(),
            project_id: runtime.project_id.clone(),
            state_root: runtime
                .dir
                .parent()
                .and_then(|path| path.parent())
                .unwrap_or(&runtime.dir)
                .to_path_buf(),
        }
    }

    fn environment(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<String, SubstrateFault> {
        #[derive(serde::Deserialize)]
        struct Environment {
            project_id: String,
            environment: String,
        }
        let record: Environment = serde_json::from_str(&checkpoint.payload)
            .map_err(|_| invalid("unreadable Stripe environment ownership record"))?;
        if self.project_id.as_deref() != Some(record.project_id.as_str())
            || record.environment != instance.resource_namespace
            || checkpoint.resource_id != record.environment
        {
            return Err(invalid(
                "Stripe environment ownership does not match the bound project and instance",
            ));
        }
        Ok(record.environment)
    }
}

fn invalid(detail: &str) -> SubstrateFault {
    SubstrateFault::from_fault(&stackless_core::state::StateError::ResourceInvariant {
        detail: detail.into(),
    })
}

fn stripe_error(error: stackless_stripe_projects::ProjectsError) -> SubstrateFault {
    SubstrateFault::from_fault(&error)
}

#[async_trait::async_trait]
impl Substrate for ManagedSubstrate {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn can_manage_resource(&self, resource: &ResourceRecord) -> bool {
        (resource.provider == "controller"
            && matches!(
                resource.resource_kind.as_str(),
                super::remote::SUBMISSION_KIND | crate::verify::command::KIND
            ))
            || self.inner.can_manage_resource(resource)
    }
    fn capabilities(&self) -> stackless_core::capabilities::Capabilities {
        self.inner.capabilities()
    }

    fn validate(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        self.inner.validate(def)
    }
    fn execution_plan(
        &self,
        def: &StackDef,
    ) -> Result<stackless_core::engine::plan::ExecutionPlan, stackless_core::def::DefError> {
        self.inner.execution_plan(def)
    }
    fn supports_source_override_for(&self, def: &StackDef, service: &str) -> bool {
        self.inner.supports_source_override_for(def, service)
    }
    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        self.inner.validate_definition(def)
    }
    fn supports_source_override(&self) -> bool {
        self.inner.supports_source_override()
    }
    fn default_lease(&self) -> Duration {
        self.inner.default_lease()
    }
    fn service_origin(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
    ) -> String {
        self.inner.service_origin(def, instance, service)
    }
    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        purpose: NamespacePurpose,
    ) -> Namespace {
        self.inner
            .build_namespace(def, instance, prior, secrets, purpose)
    }
    fn step_revision(&self, ctx: &StepContext<'_>) -> Result<String, SubstrateFault> {
        self.inner.step_revision(ctx)
    }
    fn refresh_each_operation(&self, step: &stackless_core::engine::Step) -> bool {
        self.inner.refresh_each_operation(step)
    }
    async fn reconcile(
        &self,
        ctx: StepContext<'_>,
        previous: &Checkpoint,
    ) -> Result<StepResource, SubstrateFault> {
        let mut parents = ctx.parent_resources.to_vec();
        if self.project_id.is_some() {
            parents.push(super::runtime::ENVIRONMENT_KEY);
        }
        self.inner
            .reconcile(
                StepContext {
                    parent_resources: &parents,
                    ..ctx
                },
                previous,
            )
            .await
    }
    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        let mut parents = ctx.parent_resources.to_vec();
        if self.project_id.is_some() {
            parents.push(super::runtime::ENVIRONMENT_KEY);
        }
        self.inner
            .execute(StepContext {
                parent_resources: &parents,
                ..ctx
            })
            .await
    }
    async fn observe(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<Observation, SubstrateFault> {
        if checkpoint.resource_kind != ENVIRONMENT_KIND {
            return self.inner.observe(instance, checkpoint).await;
        }
        let environment = self.environment(instance, checkpoint)?;
        let stripe = StripeProjects::new(TokioRunner, &self.stripe_dir);
        let exists = project::environment_registered(&stripe, &environment)
            .await
            .map_err(stripe_error)?;
        Ok(stackless_core::substrate::present_or_gone(exists))
    }
    async fn destroy(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        if checkpoint.resource_kind != ENVIRONMENT_KIND {
            return self.inner.destroy(instance, checkpoint).await;
        }
        let environment = self.environment(instance, checkpoint)?;
        let stripe = StripeProjects::new(TokioRunner, &self.stripe_dir);
        if project::environment_registered(&stripe, &environment)
            .await
            .map_err(stripe_error)?
        {
            project::delete_environment(&stripe, &environment)
                .await
                .map_err(stripe_error)?;
        }
        Ok(())
    }

    async fn destroy_record(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
        resource: &ResourceRecord,
    ) -> Result<(), SubstrateFault> {
        if resource.resource_kind == crate::verify::command::KIND {
            return crate::verify::command::destroy(&self.state_root, store, instance, resource);
        }
        if resource.resource_kind == super::remote::SUBMISSION_KIND {
            let path = super::remote::submission_path(&self.state_root, instance.id, resource)
                .map_err(|error| SubstrateFault::from_fault(&error))?;
            match std::fs::remove_dir_all(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => return Err(invalid("cannot remove controller submission")),
            }
            return Ok(());
        }
        if resource.resource_kind == ENVIRONMENT_KIND && resource.phase == ResourcePhase::Intent {
            let name = self.environment(instance, &resource.checkpoint(instance.name))?;
            let payload: super::runtime::EnvironmentPayload =
                serde_json::from_str(&resource.payload)
                    .map_err(|_| invalid("unreadable environment creation state"))?;
            if !payload.creation_submitted {
                store
                    .resource_absent(instance.id, &resource.key)
                    .map_err(|error| SubstrateFault::from_fault(&error))?;
                return Ok(());
            }
            let stripe = StripeProjects::new(TokioRunner, &self.stripe_dir);
            if !project::environment_registered(&stripe, &name)
                .await
                .map_err(stripe_error)?
            {
                return Err(stripe_error(
                    stackless_stripe_projects::ProjectsError::CreationUnknown { resource: name },
                ));
            }
            store
                .resource_created(
                    instance.id,
                    &resource.key,
                    &resource.resource_id,
                    &resource.payload,
                )
                .map_err(|error| SubstrateFault::from_fault(&error))?;
        }
        if stackless_integrations::is_integration_resource(&resource.resource_kind) {
            let stripe = StripeProjects::new(TokioRunner, &self.stripe_dir);
            return stackless_stripe_projects::journal::destroy_record(&stripe, store, resource)
                .await
                .map_err(stripe_error);
        }
        let current = store
            .resource(instance.id, &resource.key)
            .map_err(|error| SubstrateFault::from_fault(&error))?
            .ok_or_else(|| invalid("teardown lost its resource record"))?;
        if current.phase == ResourcePhase::Absent {
            return Ok(());
        }
        if current.resource_kind == ENVIRONMENT_KIND {
            self.destroy(instance, &current.checkpoint(instance.name))
                .await
        } else {
            self.inner.destroy_record(store, instance, &current).await
        }
    }

    async fn observe_record(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
        resource: &ResourceRecord,
    ) -> Result<Observation, SubstrateFault> {
        if resource.resource_kind == crate::verify::command::KIND {
            return crate::verify::command::observe(&self.state_root, store, instance, resource);
        }
        let current = store
            .resource(instance.id, &resource.key)
            .map_err(|error| SubstrateFault::from_fault(&error))?
            .ok_or_else(|| invalid("observation lost its resource record"))?;
        if current.phase == ResourcePhase::Absent {
            return Ok(Observation::Gone);
        }
        if current.resource_kind == super::remote::SUBMISSION_KIND {
            let path = super::remote::submission_path(&self.state_root, instance.id, &current)
                .map_err(|error| SubstrateFault::from_fault(&error))?;
            return Ok(stackless_core::substrate::present_or_gone(path.exists()));
        }
        if current.resource_kind == ENVIRONMENT_KIND {
            self.observe(instance, &current.checkpoint(instance.name))
                .await
        } else if stackless_integrations::is_integration_resource(&current.resource_kind) {
            let stripe = StripeProjects::new(TokioRunner, &self.stripe_dir);
            stackless_stripe_projects::journal::observe_payload(&stripe, &current.payload)
                .await
                .map_err(stripe_error)
        } else {
            self.inner.observe_record(store, instance, &current).await
        }
    }

    async fn restore_routes(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        self.inner.restore_routes(store, instance).await
    }
    // Environment deletion is an owned inventory entry, independently observed
    // by the engine. Provider finalizers cannot bypass that evidence.
    async fn finalize_teardown(
        &self,
        _instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        Ok(())
    }
    async fn spend(&self) -> Option<SpendInfo> {
        self.inner.spend().await
    }
    async fn fetch_logs(
        &self,
        store: &stackless_core::state::Store,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        services: &[String],
        tail: usize,
    ) -> Result<Option<Vec<ServiceLog>>, SubstrateFault> {
        self.inner
            .fetch_logs(store, def, instance, services, tail)
            .await
    }
}
