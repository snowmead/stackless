//! Persist catalog creation before sending it. An unknown result cannot authorize another create.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stackless_core::state::{Ownership, ResourceIntent, ResourcePhase, ResourceRecord, Store};
use stackless_core::substrate::{StepContext, StepResource};

use crate::ProjectsError;
use std::time::Duration;

const REMOVAL_BUDGET: Duration = Duration::from_secs(120);
const REMOVAL_POLL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct ResourceJournal {
    store: Store,
    owner: String,
    namespace: String,
    step: String,
    provider: String,
    kind: String,
    parents: Vec<String>,
    shared_catalog: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Creation {
    pub reference: String,
    pub requested_name: String,
    pub config: Value,
    pub submitted: bool,
    pub confirmed: bool,
    pub response: Value,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub remote_id: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub removal_submitted: bool,
}

#[derive(Debug)]
pub struct Attempt {
    pub record: ResourceRecord,
    pub creation: Creation,
}

fn state(error: stackless_core::state::StateError) -> ProjectsError {
    ProjectsError::Journal {
        detail: error.to_string(),
    }
}
fn encode(value: &impl Serialize) -> Result<String, ProjectsError> {
    serde_json::to_string(value).map_err(|error| ProjectsError::Journal {
        detail: error.to_string(),
    })
}
fn payload(name: &str, creation: &Creation) -> Result<String, ProjectsError> {
    encode(&json!({"stripe_resource": name, "outputs": {}, "_catalog_creation": creation}))
}

impl ResourceJournal {
    pub fn new(ctx: &StepContext<'_>, provider: &str, kind: &str) -> Self {
        Self {
            store: ctx.store.clone(),
            owner: ctx.instance.id.into(),
            namespace: ctx.instance.resource_namespace.into(),
            step: ctx.step.id.clone(),
            provider: provider.into(),
            kind: kind.into(),
            shared_catalog: None,
            parents: ctx
                .parent_resources
                .iter()
                .map(|key| (*key).into())
                .collect(),
        }
    }

    /// Account enablement can be shared even when the catalog calls it deployable.
    pub fn share_catalog(mut self, reference: &str) -> Self {
        self.shared_catalog = Some(reference.into());
        self
    }

    pub fn shared_catalog(&self, reference: &str) -> bool {
        self.shared_catalog.as_deref() == Some(reference)
    }

    pub fn begin(
        &self,
        reference: &str,
        name: &str,
        config: &Value,
        shared: bool,
    ) -> Result<Attempt, ProjectsError> {
        if (!shared || self.shared_catalog(reference))
            && !name.starts_with(&format!("{}-", self.namespace))
        {
            return Err(ProjectsError::Journal {
                detail: format!("resource {name:?} is outside the immutable instance namespace"),
            });
        }
        let key = format!("catalog:{reference}:{name}");
        let ownership = if shared {
            Ownership::Shared
        } else {
            Ownership::Owned
        };
        let step = if shared {
            stackless_core::state::INSTANCE_RESOURCE_STEP
        } else {
            &self.step
        };
        let kind = if shared && !self.shared_catalog(reference) {
            "stripe-plan"
        } else {
            &self.kind
        };
        let provider = if shared { "stripe" } else { &self.provider };
        let creation = Creation {
            reference: reference.into(),
            requested_name: name.into(),
            config: config.clone(),
            submitted: false,
            confirmed: false,
            response: Value::Null,
            project_id: self
                .store
                .instance_stripe_project(&self.owner)
                .map_err(state)?
                .and_then(|binding| binding.project_id),
            remote_id: None,
            provider_id: None,
            removal_submitted: false,
        };
        let existing = self.store.resource(&self.owner, &key).map_err(state)?;
        let record = if let Some(record) = existing {
            record
        } else {
            let inventory = self.store.resources(&self.owner).map_err(state)?;
            let mut parents: Vec<String> = self
                .parents
                .iter()
                .filter(|key| {
                    !shared
                        || inventory.iter().any(|record| {
                            record.key == **key
                                && record.step_id == stackless_core::state::INSTANCE_RESOURCE_STEP
                        })
                })
                .cloned()
                .collect();
            if !shared {
                parents.extend(
                    inventory
                        .iter()
                        .filter(|record| {
                            record.resource_kind == "stripe-plan"
                                && record.ownership == Ownership::Shared
                                && record.phase != ResourcePhase::Absent
                        })
                        .map(|record| record.key.clone()),
                );
            }
            parents.sort();
            parents.dedup();
            self.store
                .resource_intent(ResourceIntent {
                    owner_id: &self.owner,
                    key: &key,
                    step_id: step,
                    provider,
                    ownership,
                    resource_kind: kind,
                    resource_id: name,
                    payload: &payload(name, &creation)?,
                    dependencies: &parents.iter().map(String::as_str).collect::<Vec<_>>(),
                })
                .map_err(state)?
        };
        if record.provider != provider
            || record.step_id != step
            || record.resource_kind != kind
            || record.ownership != ownership
            || record.phase == ResourcePhase::Absent
        {
            return Err(ProjectsError::Journal {
                detail: format!("resource {name:?} has a different or retired ownership contract"),
            });
        }
        let value: Value =
            serde_json::from_str(&record.payload).map_err(|error| ProjectsError::Journal {
                detail: error.to_string(),
            })?;
        let creation: Creation = serde_json::from_value(value["_catalog_creation"].clone())
            .map_err(|error| ProjectsError::Journal {
                detail: error.to_string(),
            })?;
        if creation.reference != reference
            || creation.requested_name != name
            || creation.config != *config
        {
            return Err(ProjectsError::Journal {
                detail: format!(
                    "catalog configuration changed for {name:?}; this adapter has no configuration update operation"
                ),
            });
        }
        Ok(Attempt { record, creation })
    }

    pub fn submitted(&self, attempt: &mut Attempt) -> Result<(), ProjectsError> {
        attempt.creation.submitted = true;
        self.store
            .resource_intent_payload(
                &self.owner,
                &attempt.record.key,
                &payload(&attempt.record.resource_id, &attempt.creation)?,
            )
            .map_err(state)
    }

    pub async fn ensure_available<R: crate::CommandRunner>(
        &self,
        stripe: &crate::StripeProjects<R>,
        attempt: &Attempt,
    ) -> Result<(), ProjectsError> {
        let project = attempt.creation.project_id.as_deref().ok_or_else(|| {
            crate::remote::invalid("creation requires a persisted Stripe project binding")
        })?;
        if crate::remote::list(stripe, project)
            .await?
            .iter()
            .any(|resource| resource.name.as_deref() == Some(&attempt.creation.requested_name))
        {
            self.decline_preexisting(attempt)?;
            return Err(crate::remote::invalid(
                "remote name existed before this owner submitted creation",
            ));
        }
        Ok(())
    }

    pub async fn confirm_existing<R: crate::CommandRunner>(
        &self,
        stripe: &crate::StripeProjects<R>,
        attempt: &mut Attempt,
        name: &str,
    ) -> Result<(), ProjectsError> {
        let remote = if attempt.creation.remote_id.is_some() {
            verified_remote(stripe, &attempt.creation).await?
        } else {
            recover_remote(stripe, &attempt.creation, name).await?
        };
        if remote.observation()? != stackless_core::substrate::Observation::Present {
            return Err(ProjectsError::CreationUnknown {
                resource: name.into(),
            });
        }
        let mut response = attempt.creation.response.clone();
        if !response.is_object() {
            response = json!({});
        }
        response["service"] = json!({"key": remote.id, "provider_id": remote.provider});
        self.created(attempt, name, response)
    }

    pub fn created(
        &self,
        attempt: &mut Attempt,
        name: &str,
        response: Value,
    ) -> Result<(), ProjectsError> {
        attempt.creation.confirmed = true;
        if let Some(id) = response
            .pointer("/service/key")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            if attempt
                .creation
                .remote_id
                .as_deref()
                .is_some_and(|existing| existing != id)
            {
                return Err(ProjectsError::Journal {
                    detail: "catalog response changed the remote resource ID".into(),
                });
            }
            attempt.creation.remote_id = Some(id.into());
            attempt.creation.provider_id = response
                .pointer("/service/provider_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        attempt.creation.response = response;
        let current = self
            .store
            .resource(&self.owner, &attempt.record.key)
            .map_err(state)?
            .ok_or_else(|| crate::remote::invalid("catalog record disappeared"))?;
        let mut value: Value = serde_json::from_str(&current.payload)
            .map_err(|_| crate::remote::invalid("invalid catalog resource payload"))?;
        value["stripe_resource"] = json!(name);
        value["_catalog_creation"] = serde_json::to_value(&attempt.creation)
            .map_err(|_| crate::remote::invalid("invalid catalog creation state"))?;
        let updated = encode(&value)?;
        if attempt.record.phase == ResourcePhase::Ready {
            self.store
                .resource_refresh_payload(&self.owner, &attempt.record.key, name, &updated)
                .map_err(state)?;
        } else {
            self.store
                .resource_created(&self.owner, &attempt.record.key, name, &updated)
                .map_err(state)?;
            attempt.record.phase = ResourcePhase::Created;
        }
        attempt.record.resource_id = name.into();
        Ok(())
    }

    pub fn decline_preexisting(&self, attempt: &Attempt) -> Result<(), ProjectsError> {
        // No create was sent. This intent conveys no ownership of the existing object.
        self.store
            .resource_absent(&self.owner, &attempt.record.key)
            .map_err(state)
    }

    /// Save credentials and provider handles before post-provision configuration.
    pub fn outputs(&self, resource: &StepResource, ready: bool) -> Result<String, ProjectsError> {
        let records = self.store.resources(&self.owner).map_err(state)?;
        let record = records
            .iter()
            .find(|record| {
                record.step_id
                    == if self.shared_catalog.is_some() {
                        stackless_core::state::INSTANCE_RESOURCE_STEP
                    } else {
                        &self.step
                    }
                    && record.resource_kind == resource.resource_kind
                    && record.resource_id == resource.resource_id
                    && record.ownership
                        == if self.shared_catalog.is_some() {
                            Ownership::Shared
                        } else {
                            Ownership::Owned
                        }
            })
            .ok_or_else(|| ProjectsError::Journal {
                detail: "integration returned an unjournaled resource".into(),
            })?;
        let previous: Value =
            serde_json::from_str(&record.payload).map_err(|error| ProjectsError::Journal {
                detail: error.to_string(),
            })?;
        let mut value: Value =
            serde_json::from_str(&resource.payload).map_err(|error| ProjectsError::Journal {
                detail: error.to_string(),
            })?;
        value["_catalog_creation"] = previous["_catalog_creation"].clone();
        if record.phase != ResourcePhase::Ready {
            self.store
                .resource_created(
                    &self.owner,
                    &record.key,
                    &resource.resource_id,
                    &encode(&value)?,
                )
                .map_err(state)?;
        } else {
            self.store
                .resource_refresh_payload(
                    &self.owner,
                    &record.key,
                    &resource.resource_id,
                    &encode(&value)?,
                )
                .map_err(state)?;
        }
        if ready {
            self.store
                .resource_ready(&self.owner, &record.key)
                .map_err(state)?;
        }
        encode(&value)
    }
}

/// Resolve unfinished creation before teardown. Registry lag after submission is unknown.
pub async fn recover_for_teardown<R: crate::stripe::CommandRunner>(
    stripe: &crate::stripe::StripeProjects<R>,
    store: &Store,
    resource: &ResourceRecord,
) -> Result<(), ProjectsError> {
    let mut value: Value =
        serde_json::from_str(&resource.payload).map_err(|error| ProjectsError::Journal {
            detail: error.to_string(),
        })?;
    let Some(creation) = value.get("_catalog_creation") else {
        return Ok(());
    };
    let mut creation: Creation =
        serde_json::from_value(creation.clone()).map_err(|error| ProjectsError::Journal {
            detail: error.to_string(),
        })?;
    if creation.remote_id.is_some() {
        return Ok(());
    }
    if !creation.submitted {
        store
            .resource_absent(&resource.owner_id, &resource.key)
            .map_err(state)?;
        return Ok(());
    }
    let remote = recover_remote(stripe, &creation, &resource.resource_id).await?;
    creation.confirmed = true;
    creation.remote_id = Some(remote.id);
    creation.provider_id = Some(remote.provider);
    value["_catalog_creation"] =
        serde_json::to_value(&creation).map_err(|error| ProjectsError::Journal {
            detail: error.to_string(),
        })?;
    store
        .resource_created(
            &resource.owner_id,
            &resource.key,
            &resource.resource_id,
            &encode(&value)?,
        )
        .map_err(state)
}

async fn recover_remote<R: crate::CommandRunner>(
    stripe: &crate::StripeProjects<R>,
    creation: &Creation,
    name: &str,
) -> Result<crate::remote::Resource, ProjectsError> {
    let project = creation.project_id.as_deref().ok_or_else(|| {
        crate::remote::invalid("resource has no persisted Stripe project binding")
    })?;
    let catalog = stripe.catalog_for_reference(&creation.reference).await?;
    let service = catalog
        .lookup(&creation.reference)
        .ok_or_else(|| crate::remote::invalid("resource catalog identity is unavailable"))?;
    let resources = crate::remote::list(stripe, project).await?;
    let matches: Vec<_> = resources
        .into_iter()
        .filter(|resource| resource.name.as_deref() == Some(name))
        .collect();
    if matches.len() != 1 {
        return Err(ProjectsError::CreationUnknown {
            resource: name.into(),
        });
    }
    let remote = matches
        .into_iter()
        .next()
        .ok_or_else(|| ProjectsError::CreationUnknown {
            resource: name.into(),
        })?;
    if remote.provider != service.provider_id || remote.service_ref != service.service_id {
        return Err(crate::remote::invalid(
            "remote resource name has a different catalog identity",
        ));
    }
    Ok(remote)
}

fn creation_from_payload(payload: &str) -> Result<Creation, ProjectsError> {
    let value: Value = serde_json::from_str(payload)
        .map_err(|_| crate::remote::invalid("invalid catalog resource payload"))?;
    serde_json::from_value(value["_catalog_creation"].clone())
        .map_err(|_| crate::remote::invalid("catalog resource has no durable remote identity"))
}

pub async fn observe_payload<R: crate::CommandRunner>(
    stripe: &crate::StripeProjects<R>,
    payload: &str,
) -> Result<stackless_core::substrate::Observation, ProjectsError> {
    let creation = creation_from_payload(payload)?;
    verified_remote(stripe, &creation).await?.observation()
}

async fn verified_remote<R: crate::CommandRunner>(
    stripe: &crate::StripeProjects<R>,
    creation: &Creation,
) -> Result<crate::remote::Resource, ProjectsError> {
    let id = creation
        .remote_id
        .as_deref()
        .ok_or_else(|| crate::remote::invalid("catalog resource remote ID is unresolved"))?;
    let remote = crate::remote::get(stripe, id).await?;
    if remote.name.as_deref() != Some(&creation.requested_name)
        || creation.provider_id.as_deref() != Some(&remote.provider)
        || creation
            .reference
            .split_once('/')
            .map(|(_, service)| service)
            != Some(remote.service_ref.as_str())
    {
        return Err(crate::remote::invalid(
            "remote resource no longer matches the recorded catalog identity",
        ));
    }
    Ok(remote)
}

pub async fn destroy_record<R: crate::CommandRunner>(
    stripe: &crate::StripeProjects<R>,
    store: &Store,
    resource: &ResourceRecord,
) -> Result<(), ProjectsError> {
    if resource.ownership != Ownership::Owned {
        return Err(crate::remote::invalid(
            "cannot remove a borrowed or shared catalog resource",
        ));
    }
    recover_for_teardown(stripe, store, resource).await?;
    let current = store
        .resource(&resource.owner_id, &resource.key)
        .map_err(state)?
        .ok_or_else(|| crate::remote::invalid("catalog resource record disappeared"))?;
    if current.phase == ResourcePhase::Absent {
        return Ok(());
    }
    let mut creation = creation_from_payload(&current.payload)?;
    // A fresh read verifies the exact remote identity before sending deletion.
    let remote = verified_remote(stripe, &creation).await?;
    if remote.status == "removed" {
        return Ok(());
    }
    if creation.removal_submitted && remote.status == "pending" {
        return wait_for_removal(stripe, &creation, REMOVAL_BUDGET).await;
    }
    creation.removal_submitted = true;
    let mut payload: Value = serde_json::from_str(&current.payload)
        .map_err(|_| crate::remote::invalid("invalid catalog resource payload"))?;
    payload["_catalog_creation"] = serde_json::to_value(&creation)
        .map_err(|_| crate::remote::invalid("invalid catalog removal state"))?;
    store
        .resource_refresh_payload(
            &current.owner_id,
            &current.key,
            &current.resource_id,
            &encode(&payload)?,
        )
        .map_err(state)?;
    let removal = crate::remote::remove(
        stripe,
        creation
            .remote_id
            .as_deref()
            .ok_or_else(|| crate::remote::invalid("remote resource ID is unresolved"))?,
    )
    .await?;
    match removal {
        crate::remote::Removal::Removed => Ok(()),
        crate::remote::Removal::Pending => {
            wait_for_removal(stripe, &creation, REMOVAL_BUDGET).await
        }
    }
}

async fn wait_for_removal<R: crate::CommandRunner>(
    stripe: &crate::StripeProjects<R>,
    creation: &Creation,
    budget: Duration,
) -> Result<(), ProjectsError> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let remote = verified_remote(stripe, creation).await?;
        match remote.status.as_str() {
            "removed" => return Ok(()),
            // A replica can still report the pre-removal state after acceptance.
            "pending" | "complete" => {}
            _ => {
                return Err(crate::remote::invalid(format!(
                    "remote removal has unresolved status {:?}",
                    remote.status
                )));
            }
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(crate::remote::invalid(
                "remote resource removal is still pending after the deadline",
            ));
        }
        tokio::time::sleep(REMOVAL_POLL.min(remaining)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stripe::{CommandOutput, CommandRunner, StripeProjects};
    use crate::test_support::ok;
    use stackless_core::def::StackDef;
    use stackless_core::engine::{Step, StepKind};
    use stackless_core::state::InstanceRecord;
    use stackless_core::substrate::InstanceContext;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct External {
        names: Vec<String>,
        creates: usize,
        failure: Option<&'static str>,
        hidden: bool,
        deletes: usize,
        remote_status: Option<&'static str>,
        local_hidden: bool,
        delayed_removal_reads: usize,
    }
    struct Runner {
        db: PathBuf,
        owner: String,
        external: Arc<Mutex<External>>,
    }

    #[async_trait::async_trait]
    impl CommandRunner for Runner {
        async fn run(&self, args: &[String], _cwd: &Path) -> Result<CommandOutput, ProjectsError> {
            let store = Store::open(&self.db).unwrap();
            let records = store.resources(&self.owner).unwrap();
            let record = records
                .first()
                .expect("intent must precede every provider request");
            let mut external = self.external.lock().unwrap();
            match args[0].as_str() {
                "catalog" => {
                    let mut catalog: Value =
                        serde_json::from_str(include_str!("../tests/fixtures/catalog.json"))
                            .unwrap();
                    let mut service = catalog["data"]["services"][0].clone();
                    service["provider_name"] = json!("Example");
                    service["provider_id"] = json!("provider_1");
                    service["service_id"] = json!("database");
                    catalog["data"]["services"] = json!([service]);
                    Ok(ok(catalog["data"].clone()))
                }
                "services" => Ok(ok(
                    json!({"services": if external.hidden || external.local_hidden { vec![] } else {
                    external.names.iter().map(|name| json!({"name": name, "service_id": "database", "provider_name": "Example"})).collect::<Vec<_>>()
                }, "plans": []}),
                )),
                "add" => {
                    assert_eq!(record.phase, ResourcePhase::Intent);
                    let value: Value = serde_json::from_str(&record.payload).unwrap();
                    assert_eq!(value["_catalog_creation"]["submitted"], true);
                    let name =
                        args[args.iter().position(|arg| arg == "--name").unwrap() + 1].clone();
                    external.names.push(name);
                    external.creates += 1;
                    if external.failure == Some("response") {
                        external.failure = None;
                        return Err(ProjectsError::Unavailable {
                            detail: "response lost after provider creation".into(),
                        });
                    }
                    Ok(ok(
                        json!({"service": {"key": "remote-id-1", "provider_id": "provider_1"}, "SECRET_KEY": "app-secret-canary"}),
                    ))
                }
                "env" => {
                    assert_eq!(record.phase, ResourcePhase::Created);
                    if external.failure == Some("attach") {
                        external.failure = None;
                        let value: Value = serde_json::from_str(&record.payload).unwrap();
                        assert_eq!(value["_catalog_creation"]["remote_id"], "remote-id-1");
                        return Err(ProjectsError::Unavailable {
                            detail: "environment attachment failed".into(),
                        });
                    }
                    Ok(ok(json!({})))
                }
                _ => panic!("unexpected call {args:?}"),
            }
        }

        async fn request(
            &self,
            method: &str,
            path: &str,
            _cwd: &Path,
        ) -> Result<CommandOutput, ProjectsError> {
            let mut external = self.external.lock().unwrap();
            let value = if method == "POST" {
                assert!(path.ends_with("/remote-id-1/remove"));
                let store = Store::open(&self.db).unwrap();
                let record = store.resources(&self.owner).unwrap().remove(0);
                let payload: Value = serde_json::from_str(&record.payload).unwrap();
                assert_eq!(payload["_catalog_creation"]["removal_submitted"], true);
                external.deletes += 1;
                external.local_hidden = true;
                external.remote_status = Some(if external.delayed_removal_reads > 0 {
                    "pending"
                } else {
                    "removed"
                });
                if external.failure == Some("delete_response") {
                    external.failure = None;
                    return Err(crate::remote::invalid("lost remote removal response"));
                }
                json!({"status": external.remote_status})
            } else {
                if external.deletes > 0 && external.remote_status == Some("pending") {
                    if external.delayed_removal_reads > 0 {
                        external.delayed_removal_reads -= 1;
                    } else {
                        external.remote_status = Some("removed");
                    }
                }
                let rows: Vec<Value> = external.names.iter().map(|name| json!({"id": "remote-id-1", "name": name, "provider": "provider_1", "service_ref": "database", "status": external.remote_status.unwrap_or("complete")})).collect();
                if path.contains('?') {
                    json!({"data": if external.hidden { vec![] } else { rows }, "next_page_url": null})
                } else {
                    assert!(path.ends_with("/remote-id-1"));
                    rows.into_iter()
                        .next()
                        .unwrap_or(json!({"error": {"code": "not_found"}}))
                }
            };
            Ok(CommandOutput {
                status: 0,
                stdout: value.to_string(),
                stderr: String::new(),
            })
        }
    }

    fn bound<'a>(
        runner: &'a Runner,
        store: &Store,
        record: &InstanceRecord,
    ) -> StripeProjects<&'a Runner> {
        let def = StackDef::parse(&record.definition).unwrap();
        let step = Step {
            id: "integration:db".into(),
            kind: StepKind::ProvisionIntegration,
            node: "db".into(),
        };
        let instance = InstanceContext::from_record(record, &[]);
        let ctx = StepContext {
            operation_id: "operation",
            store,
            instance: &instance,
            def: &def,
            step: &step,
            source_overrides: &BTreeMap::new(),
            dirty: false,
            prior: &[],
            parent_resources: &[],
            cancelled: None,
        };
        StripeProjects::new(runner, runner.db.parent().unwrap()).with_journal(
            &ctx,
            "local",
            "example-database",
        )
    }

    fn record(store: &Store) -> InstanceRecord {
        let owner = store
            .create_instance(
                "demo",
                "local",
                "[stack]\nname='demo'\n[integrations.db]\nprovider='example'",
                &BTreeMap::new(),
                "",
                false,
            )
            .unwrap();
        store
            .bind_stripe_project(&owner.instance_id, "test-project", Some("project_1"))
            .unwrap();
        owner
    }

    #[test]
    fn shared_plans_have_one_instance_reference_across_integration_steps() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("state.db")).unwrap();
        let owner = record(&store);
        let first = ResourceJournal {
            store: store.clone(),
            owner: owner.instance_id.clone(),
            namespace: owner.resource_namespace.clone(),
            step: "integration:one".into(),
            provider: "local".into(),
            kind: "one-resource".into(),
            shared_catalog: None,
            parents: vec![],
        };
        let second = ResourceJournal {
            step: "integration:two".into(),
            kind: "two-resource".into(),
            ..first.clone()
        };
        let one = first
            .begin("example/hobby", "hobby", &json!({}), true)
            .unwrap();
        let two = second
            .begin("example/hobby", "hobby", &json!({}), true)
            .unwrap();
        assert_eq!(one.record.key, two.record.key);
        assert_eq!(two.record.ownership, Ownership::Shared);
        assert_eq!(
            two.record.step_id,
            stackless_core::state::INSTANCE_RESOURCE_STEP
        );
        assert_eq!(store.resources(&owner.instance_id).unwrap().len(), 1);
    }

    #[test]
    fn account_enablement_is_an_explicit_shared_reference_with_recorded_outputs() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("state.db")).unwrap();
        let owner = record(&store);
        let name = InstanceContext::from_record(&owner, &[]).resource_name("web");
        let journal = ResourceJournal {
            store: store.clone(),
            owner: owner.instance_id.clone(),
            namespace: owner.resource_namespace.clone(),
            step: "start:web".into(),
            provider: "cloudflare".into(),
            kind: "cloudflare-hosting".into(),
            parents: vec![],
            shared_catalog: Some("cloudflare/workers".into()),
        };
        let mut attempt = journal
            .begin("cloudflare/workers", &name, &json!({}), true)
            .unwrap();
        assert_eq!(attempt.record.ownership, Ownership::Shared);
        assert_eq!(
            attempt.record.step_id,
            stackless_core::state::INSTANCE_RESOURCE_STEP
        );
        assert_eq!(attempt.record.resource_kind, "cloudflare-hosting");
        journal.submitted(&mut attempt).unwrap();
        journal
            .created(
                &mut attempt,
                &name,
                json!({"service":{"key":"remote_one","provider_id":"cloudflare"}}),
            )
            .unwrap();
        let resource = StepResource {
            resource_kind: "cloudflare-hosting".into(),
            resource_id: name.clone(),
            payload: json!({"stripe_resource":name,"outputs":{"account_id":"account_one"}})
                .to_string(),
        };
        let payload = journal.outputs(&resource, true).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&payload).unwrap()["_catalog_creation"]["remote_id"],
            "remote_one"
        );
        assert_eq!(
            store
                .resource(&owner.instance_id, &attempt.record.key)
                .unwrap()
                .unwrap()
                .ownership,
            Ownership::Shared
        );
    }

    #[tokio::test]
    async fn creation_and_attachment_failures_recover_one_resource_after_store_reopen() {
        for failure in ["response", "attach"] {
            let root = tempfile::tempdir().unwrap();
            let db = root.path().join("state.db");
            let store = Store::open(&db).unwrap();
            let owner = record(&store);
            let external = Arc::new(Mutex::new(External {
                failure: Some(failure),
                ..Default::default()
            }));
            let runner = Runner {
                db: db.clone(),
                owner: owner.instance_id.clone(),
                external: external.clone(),
            };
            let name = InstanceContext::from_record(&owner, &[]).resource_name("db");
            let stripe = bound(&runner, &store, &owner);
            assert!(
                crate::project::add_resource(&stripe, "example/database", &name, &json!({}), false)
                    .await
                    .is_err()
            );
            drop(stripe);
            drop(store);
            let store = Store::open(&db).unwrap();
            let stripe = bound(&runner, &store, &owner);
            let added =
                crate::project::add_resource(&stripe, "example/database", &name, &json!({}), false)
                    .await
                    .unwrap();
            assert_eq!(added.name, name);
            assert_eq!(external.lock().unwrap().creates, 1);
            assert_eq!(
                store.resources(&owner.instance_id).unwrap()[0].phase,
                ResourcePhase::Created
            );
            let output = StepResource { resource_kind: "example-database".into(), resource_id: name,
                payload: encode(&json!({"stripe_resource": added.name, "outputs": {"url": "app-output-canary"}})).unwrap() };
            stripe.journal().unwrap().outputs(&output, false).unwrap();
            assert!(
                store.resources(&owner.instance_id).unwrap()[0]
                    .payload
                    .contains("app-output-canary")
            );
            stripe.journal().unwrap().outputs(&output, true).unwrap();
            assert_eq!(
                store.resources(&owner.instance_id).unwrap()[0].phase,
                ResourcePhase::Ready
            );
        }
    }

    #[tokio::test]
    async fn absent_inventory_after_a_lost_response_never_authorizes_a_second_create() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("state.db");
        let store = Store::open(&db).unwrap();
        let owner = record(&store);
        let external = Arc::new(Mutex::new(External {
            failure: Some("response"),
            hidden: true,
            ..Default::default()
        }));
        let runner = Runner {
            db,
            owner: owner.instance_id.clone(),
            external: external.clone(),
        };
        let stripe = bound(&runner, &store, &owner);
        let name = InstanceContext::from_record(&owner, &[]).resource_name("db");
        assert!(
            crate::project::add_resource(&stripe, "example/database", &name, &json!({}), false)
                .await
                .is_err()
        );
        let error =
            crate::project::add_resource(&stripe, "example/database", &name, &json!({}), false)
                .await
                .unwrap_err();
        assert!(matches!(error, ProjectsError::CreationUnknown { .. }));
        assert_eq!(external.lock().unwrap().creates, 1);
        let pending = store.resources(&owner.instance_id).unwrap().remove(0);
        assert!(matches!(
            recover_for_teardown(&stripe, &store, &pending).await,
            Err(ProjectsError::CreationUnknown { .. })
        ));
        assert_eq!(
            store.resources(&owner.instance_id).unwrap()[0].phase,
            ResourcePhase::Intent
        );
        external.lock().unwrap().hidden = false;
        recover_for_teardown(&stripe, &store, &pending)
            .await
            .unwrap();
        let recovered = store.resources(&owner.instance_id).unwrap().remove(0);
        assert_eq!(recovered.phase, ResourcePhase::Created);
        let value: Value = serde_json::from_str(&recovered.payload).unwrap();
        assert_eq!(value["_catalog_creation"]["confirmed"], true);
    }

    #[tokio::test]
    async fn remote_deletion_survives_lost_response_and_missing_local_registration() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("state.db");
        let store = Store::open(&db).unwrap();
        let owner = record(&store);
        let external = Arc::new(Mutex::new(External::default()));
        let runner = Runner {
            db: db.clone(),
            owner: owner.instance_id.clone(),
            external: external.clone(),
        };
        let name = InstanceContext::from_record(&owner, &[]).resource_name("db");
        let stripe = bound(&runner, &store, &owner);
        crate::project::add_resource(&stripe, "example/database", &name, &json!({}), false)
            .await
            .unwrap();
        let resource = store.resources(&owner.instance_id).unwrap().remove(0);
        external.lock().unwrap().failure = Some("delete_response");
        assert!(destroy_record(&stripe, &store, &resource).await.is_err());
        drop(stripe);
        drop(store);
        let store = Store::open(&db).unwrap();
        let stripe = bound(&runner, &store, &owner);
        let resource = store.resources(&owner.instance_id).unwrap().remove(0);
        destroy_record(&stripe, &store, &resource).await.unwrap();
        assert_eq!(
            observe_payload(&stripe, &resource.payload).await.unwrap(),
            stackless_core::substrate::Observation::Gone
        );
        let external = external.lock().unwrap();
        assert!(external.local_hidden);
        assert_eq!(external.deletes, 1);
    }

    #[tokio::test]
    async fn pending_remote_deletion_waits_and_recovers_without_another_removal() {
        for lost_response in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let db = root.path().join("state.db");
            let store = Store::open(&db).unwrap();
            let owner = record(&store);
            let external = Arc::new(Mutex::new(External::default()));
            let runner = Runner {
                db,
                owner: owner.instance_id.clone(),
                external: external.clone(),
            };
            let name = InstanceContext::from_record(&owner, &[]).resource_name("db");
            let stripe = bound(&runner, &store, &owner);
            crate::project::add_resource(&stripe, "example/database", &name, &json!({}), false)
                .await
                .unwrap();
            let resource = store.resources(&owner.instance_id).unwrap().remove(0);
            {
                let mut state = external.lock().unwrap();
                state.delayed_removal_reads = 2;
                state.failure = lost_response.then_some("delete_response");
            }
            if lost_response {
                assert!(destroy_record(&stripe, &store, &resource).await.is_err());
            }
            destroy_record(&stripe, &store, &resource).await.unwrap();
            assert_eq!(
                observe_payload(&stripe, &resource.payload).await.unwrap(),
                stackless_core::substrate::Observation::Gone
            );
            assert_eq!(external.lock().unwrap().deletes, 1);

            // A deadline is not evidence of deletion, even after a submitted removal.
            external.lock().unwrap().remote_status = Some("complete");
            let creation =
                creation_from_payload(&store.resources(&owner.instance_id).unwrap()[0].payload)
                    .unwrap();
            assert!(creation.removal_submitted);
            assert!(
                wait_for_removal(&stripe, &creation, Duration::ZERO)
                    .await
                    .is_err()
            );
            assert_eq!(external.lock().unwrap().deletes, 1);
        }
    }

    #[tokio::test]
    async fn local_absence_and_remote_pending_or_error_never_prove_deletion() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("state.db");
        let store = Store::open(&db).unwrap();
        let owner = record(&store);
        let external = Arc::new(Mutex::new(External::default()));
        let runner = Runner {
            db,
            owner: owner.instance_id.clone(),
            external: external.clone(),
        };
        let name = InstanceContext::from_record(&owner, &[]).resource_name("db");
        let stripe = bound(&runner, &store, &owner);
        crate::project::add_resource(&stripe, "example/database", &name, &json!({}), false)
            .await
            .unwrap();
        let resource = store.resources(&owner.instance_id).unwrap().remove(0);
        for status in ["pending", "error", "unknown", "expired"] {
            {
                let mut state = external.lock().unwrap();
                state.remote_status = Some(status);
                state.local_hidden = true;
            }
            assert!(observe_payload(&stripe, &resource.payload).await.is_err());
        }
        // Failed provisioning can still leave billable state. It must be removable.
        external.lock().unwrap().remote_status = Some("error");
        destroy_record(&stripe, &store, &resource).await.unwrap();
        assert_eq!(external.lock().unwrap().deletes, 1);
    }

    #[tokio::test]
    async fn stale_local_inventory_cannot_authorize_adopting_a_remote_collision() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("state.db");
        let store = Store::open(&db).unwrap();
        let owner = record(&store);
        let name = InstanceContext::from_record(&owner, &[]).resource_name("db");
        let external = Arc::new(Mutex::new(External {
            names: vec![name.clone()],
            local_hidden: true,
            ..Default::default()
        }));
        let runner = Runner {
            db,
            owner: owner.instance_id.clone(),
            external: external.clone(),
        };
        let stripe = bound(&runner, &store, &owner);
        assert!(
            crate::project::add_resource(&stripe, "example/database", &name, &json!({}), false)
                .await
                .is_err()
        );
        assert_eq!(external.lock().unwrap().creates, 0);
        assert_eq!(
            store.resources(&owner.instance_id).unwrap()[0].phase,
            ResourcePhase::Absent
        );
    }

    #[tokio::test]
    async fn preexisting_exact_name_is_not_owned_without_a_submitted_intent() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("state.db");
        let store = Store::open(&db).unwrap();
        let owner = record(&store);
        let name = InstanceContext::from_record(&owner, &[]).resource_name("db");
        let external = Arc::new(Mutex::new(External {
            names: vec![name.clone()],
            ..Default::default()
        }));
        let runner = Runner {
            db,
            owner: owner.instance_id.clone(),
            external: external.clone(),
        };
        let stripe = bound(&runner, &store, &owner);
        let error =
            crate::project::add_resource(&stripe, "example/database", &name, &json!({}), false)
                .await
                .unwrap_err();
        assert!(matches!(error, ProjectsError::Journal { .. }));
        assert_eq!(external.lock().unwrap().creates, 0);
        assert_eq!(
            store.resources(&owner.instance_id).unwrap()[0].phase,
            ResourcePhase::Absent
        );
    }
}
