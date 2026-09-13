//! Shared Stripe project anchors and immutable per-birth bindings.

use super::row::Row;
use super::{StateError, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripeProjectRecord {
    pub scope: String,
    pub context_id: String,
    pub resource_name: String,
    pub project_id: Option<String>,
    pub creation_started: bool,
}

impl StripeProjectRecord {
    fn from_row(row: &Row) -> Result<Self, StateError> {
        Ok(Self {
            scope: row.get_string(0)?,
            context_id: row.get_string(1)?,
            resource_name: row.get_string(2)?,
            project_id: row.get_opt_string(3)?,
            creation_started: row.get_i64(4)? != 0,
        })
    }
}

impl Store {
    pub fn stripe_project(&self, scope: &str) -> Result<Option<StripeProjectRecord>, StateError> {
        self.query_row("SELECT scope, context_id, resource_name, project_id, creation_started FROM stripe_projects WHERE scope = ?1", &[scope.into()], StripeProjectRecord::from_row)
    }

    pub fn instance_stripe_project(
        &self,
        owner_id: &str,
    ) -> Result<Option<StripeProjectRecord>, StateError> {
        self.query_row("SELECT p.scope, p.context_id, p.resource_name, p.project_id, p.creation_started FROM stripe_projects p JOIN instance_stripe_projects b ON b.scope = p.scope WHERE b.owner_id = ?1", &[owner_id.into()], StripeProjectRecord::from_row)
    }

    /// The shared project intent and the instance binding precede CLI effects.
    pub fn bind_stripe_project(
        &self,
        owner_id: &str,
        scope: &str,
        project_id: Option<&str>,
    ) -> Result<StripeProjectRecord, StateError> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.execute("INSERT INTO stripe_projects (scope, context_id, resource_name, project_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(scope) DO NOTHING",
            &[scope.into(), id.clone().into(), format!("stackless-{id}").into(), project_id.map_or(super::value::Value::Null, Into::into), Self::now().into()])?;
        self.execute("INSERT INTO instance_stripe_projects (owner_id, scope) SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM instances WHERE instance_id = ?1 AND status = 'active') ON CONFLICT(owner_id) DO NOTHING", &[owner_id.into(), scope.into()])?;
        let bound = self.instance_stripe_project(owner_id)?.ok_or_else(|| {
            StateError::ResourceInvariant {
                detail: "Stripe context requires an active instance identity".into(),
            }
        })?;
        if bound.scope != scope
            || project_id.is_some_and(|id| bound.project_id.as_deref() != Some(id))
        {
            return Err(StateError::ResourceInvariant { detail: "an instance's Stripe project binding cannot change; destroy it before selecting another project".into() });
        }
        Ok(bound)
    }

    /// A lost create response must be resolved by exact-name inventory lookup.
    /// It never authorizes a second create request.
    pub fn start_stripe_project_creation(&self, scope: &str) -> Result<bool, StateError> {
        Ok(self.execute("UPDATE stripe_projects SET creation_started = 1 WHERE scope = ?1 AND project_id IS NULL AND creation_started = 0", &[scope.into()])? == 1)
    }

    pub fn stripe_project_created(&self, scope: &str, project_id: &str) -> Result<(), StateError> {
        if project_id.is_empty() || self.execute("UPDATE stripe_projects SET project_id = ?2 WHERE scope = ?1 AND (project_id IS NULL OR project_id = ?2)", &[scope.into(), project_id.into()])? != 1 {
            return Err(StateError::ResourceInvariant { detail: "Stripe project identity is missing or conflicts with its persisted anchor".into() });
        }
        Ok(())
    }
}
