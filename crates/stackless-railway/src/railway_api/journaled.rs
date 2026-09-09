//! Native handles are recovered from ownership receipts, never display names.
use super::*;
use crate::lifecycle::{Journal, NativeState, SERVICE_RECEIPT, digest};
use std::collections::BTreeSet;

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str, RailwayError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| api_failed("native identity", format!("missing {name}")))
}
fn nullable_id(value: &Value, name: &str) -> Result<Option<String>, RailwayError> {
    match value.get(name) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(id)) if valid_id(id) => Ok(Some(id.clone())),
        _ => Err(api_failed("native identity", format!("invalid {name}"))),
    }
}
fn deleted(value: &Value) -> Result<bool, RailwayError> {
    match value.get("deletedAt") {
        Some(Value::Null) => Ok(false),
        Some(Value::String(time)) if chrono::DateTime::parse_from_rfc3339(time).is_ok() => Ok(true),
        _ => Err(api_failed("native identity", "invalid deletedAt")),
    }
}
fn page(
    value: &Value,
    seen: &mut BTreeSet<String>,
) -> Result<(Vec<Value>, Option<String>), RailwayError> {
    let edges = value
        .get("edges")
        .and_then(Value::as_array)
        .ok_or_else(|| api_failed("inventory", "missing edges"))?;
    let mut result = Vec::new();
    for edge in edges {
        let node = edge
            .get("node")
            .filter(|v| v.is_object())
            .ok_or_else(|| api_failed("inventory", "missing node"))?;
        let id = scalar_id(node, "/id", "inventory")?;
        if !seen.insert(id.to_owned()) {
            return Err(api_failed("inventory", "duplicate ID"));
        }
        result.push(node.clone());
    }
    let next = match value
        .pointer("/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
    {
        Some(false) => None,
        Some(true) => Some(
            value
                .pointer("/pageInfo/endCursor")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| api_failed("inventory", "missing next cursor"))?
                .to_owned(),
        ),
        _ => return Err(api_failed("inventory", "missing page info")),
    };
    Ok((result, next))
}
impl RailwayApi {
    pub(crate) async fn checkpoint_ready(
        &self,
        payload: &crate::RailwayPayload,
    ) -> Result<bool, RailwayError> {
        if let Some(state) = &payload.native {
            if state.project_id.as_deref() != Some(&payload.project_id)
                || state.service_id.as_deref() != Some(&payload.railway_service_id)
                || state.environment_id.as_deref() != Some(&payload.environment_id)
                || !state.scope_bound
                || state.removal_submitted
                || state.absence_verified
            {
                return Err(api_failed(
                    "checkpoint",
                    "native checkpoint identity differs or teardown started",
                ));
            }
            let requests: Vec<_> = state
                .effects
                .iter()
                .filter(|(key, e)| {
                    key.ends_with(":deploy") && e.id.as_deref() == Some(&payload.deployment_id)
                })
                .collect();
            if requests.len() != 1 {
                return Err(api_failed(
                    "checkpoint",
                    "deployment has no unique submitted intent",
                ));
            }
            let (key, effect) = requests[0];
            if !effect.submitted || !effect.confirmed {
                return Err(api_failed("checkpoint", "deployment intent is incomplete"));
            }
            let revision = key
                .strip_suffix(":deploy")
                .ok_or_else(|| api_failed("checkpoint", "deployment revision missing"))?;
            let data = self.graphql("project", r#"query OwnedProject($id:String!){project(id:$id){id name description workspaceId deletedAt}}"#, json!({"id":payload.project_id})).await?;
            if Self::verify_project(&data["project"], state)? {
                return Ok(false);
            }
            let data = self
                .graphql(
                    "service",
                    r#"query OwnedService($id:String!){service(id:$id){id name projectId}}"#,
                    json!({"id":payload.railway_service_id}),
                )
                .await?;
            if scalar_id(&data, "/service/id", "service")? != payload.railway_service_id
                || scalar_id(&data, "/service/projectId", "service")? != payload.project_id
                || Some(field(&data["service"], "name")?) != state.service_name.as_deref()
            {
                return Err(api_failed(
                    "service",
                    "checkpoint service ownership differs",
                ));
            }
            let variables = self
                .variables(
                    &payload.project_id,
                    &payload.railway_service_id,
                    &payload.environment_id,
                )
                .await?;
            if variables.get(SERVICE_RECEIPT).and_then(Value::as_str)
                != state.project_receipt.as_deref()
            {
                return Err(api_failed("service", "service receipt differs"));
            }
            let instance = self
                .service_instance(&payload.railway_service_id, &payload.environment_id)
                .await?;
            let root = field(&instance, "rootDirectory")?
                .trim_matches('/')
                .trim_start_matches("./");
            let input = json!({"source":instance.get("source"),"rootDirectory":format!("/{root}"),"startCommand":instance.get("startCommand")});
            let settings = state
                .effects
                .get(&format!("{revision}:settings"))
                .ok_or_else(|| api_failed("checkpoint", "settings intent missing"))?;
            let vars = state
                .effects
                .get(&format!("{revision}:variables"))
                .ok_or_else(|| api_failed("checkpoint", "variables intent missing"))?;
            if !settings.confirmed || !vars.confirmed {
                return Err(api_failed("checkpoint", "configuration intent incomplete"));
            }
            if settings.fingerprint
                != digest(&(&payload.railway_service_id, &payload.environment_id, &input))?
                || vars.fingerprint
                    != digest(&(
                        &payload.project_id,
                        &payload.railway_service_id,
                        &payload.environment_id,
                        &variables,
                    ))?
            {
                return Ok(false);
            }
            let domains = self
                .domain_inventory(
                    &payload.project_id,
                    &payload.railway_service_id,
                    &payload.environment_id,
                )
                .await?;
            let domain = state
                .effects
                .get("domain-create")
                .and_then(|e| e.id.as_deref())
                .ok_or_else(|| api_failed("checkpoint", "domain ID missing"))?;
            if domains.len() != 1 || scalar_id(&domains[0], "/id", "domains")? != domain {
                return Ok(false);
            }
            let host = field(&domains[0], "domain")?;
            let origin = if host.contains("://") {
                host.to_owned()
            } else {
                format!("https://{host}")
            };
            if origin.trim_end_matches('/') != payload.origin {
                return Ok(false);
            }
        }
        self.deployment_revision_ready(
            &payload.project_id,
            &payload.railway_service_id,
            &payload.environment_id,
            &payload.deployment_id,
            payload.commit_sha.as_deref(),
        )
        .await
    }

    async fn projects_inventory(&self) -> Result<Vec<Value>, RailwayError> {
        let mut rows = Vec::new();
        let mut cursor = None;
        let mut cursors = BTreeSet::new();
        let mut seen = BTreeSet::new();
        loop {
            let data=self.graphql("projects",r#"query OwnedProjects($after:String){projects(includeDeleted:true,first:100,after:$after){edges{node{id name description workspaceId deletedAt}}pageInfo{hasNextPage endCursor}}}"#,json!({"after":cursor})).await?;
            let (nodes, next) = page(&data["projects"], &mut seen)?;
            for node in &nodes {
                field(node, "name")?;
                nullable_id(node, "workspaceId")?;
                deleted(node)?;
                if !matches!(
                    node.get("description"),
                    Some(Value::Null | Value::String(_))
                ) {
                    return Err(api_failed(
                        "projects",
                        "project description missing or malformed",
                    ));
                }
            }
            rows.extend(nodes);
            if let Some(next) = next {
                if !cursors.insert(next.clone()) {
                    return Err(api_failed("projects", "pagination cursor repeated"));
                }
                cursor = Some(next);
            } else {
                return Ok(rows);
            }
        }
    }
    fn verify_project(node: &Value, state: &NativeState) -> Result<bool, RailwayError> {
        let id = scalar_id(node, "/id", "project")?;
        if state.project_id.as_deref().is_some_and(|s| s != id)
            || Some(field(node, "name")?) != state.project_name.as_deref()
            || Some(field(node, "description")?) != state.project_receipt.as_deref()
        {
            return Err(api_failed("project", "project ownership receipt differs"));
        }
        let workspace = nullable_id(node, "workspaceId")?;
        if state.scope_bound && state.workspace_id != workspace {
            return Err(api_failed("project", "workspace differs"));
        }
        deleted(node)
    }
    pub(crate) async fn native_project_deleted(
        &self,
        journal: &Journal,
    ) -> Result<bool, RailwayError> {
        let state = journal.load()?;
        if state.absence_verified {
            return Ok(true);
        }
        let Some(effect) = state.effects.get("project-create") else {
            return Ok(true);
        };
        if !effect.submitted {
            return Ok(true);
        }
        let rows = self.projects_inventory().await?;
        let candidates: Vec<_> = rows
            .iter()
            .filter(|v| {
                if let Some(id) = state.project_id.as_ref().or(effect.id.as_ref()) {
                    v.get("id").and_then(Value::as_str) == Some(id)
                } else {
                    v.get("description").and_then(Value::as_str) == state.project_receipt.as_deref()
                }
            })
            .collect();
        if candidates.len() != 1 {
            return Err(api_failed(
                "projects",
                "native identity or deletion is unresolved; retaining ownership",
            ));
        }
        let node = candidates[0];
        let gone = Self::verify_project(node, &state)?;
        let id = scalar_id(node, "/id", "project")?;
        journal.identify("project-create", id)?;
        journal.project(id, nullable_id(node, "workspaceId")?.as_deref())?;
        Ok(gone)
    }
    pub(crate) async fn remove_native_project(
        &self,
        journal: &Journal,
    ) -> Result<(), RailwayError> {
        if self.native_project_deleted(journal).await? {
            let mut state = journal.load()?;
            state.absence_verified = true;
            journal.save(&state)?;
            return Ok(());
        }
        let mut state = journal.load()?;
        let id = state
            .project_id
            .clone()
            .ok_or_else(|| api_failed("projectDelete", "project ID missing"))?;
        if !state.removal_submitted {
            state.removal_submitted = true;
            journal.save(&state)?;
            let data = self
                .graphql(
                    "projectDelete",
                    r#"mutation ProjectDelete($id:String!){projectDelete(id:$id)}"#,
                    json!({"id":id}),
                )
                .await?;
            if data.get("projectDelete").and_then(Value::as_bool) != Some(true) {
                return Err(api_failed("projectDelete", "mutation did not return true"));
            }
        }
        if !self.native_project_deleted(journal).await? {
            return Err(api_failed(
                "projectDelete",
                "native deletion unconfirmed; retaining ownership",
            ));
        }
        let mut state = journal.load()?;
        state.absence_verified = true;
        journal.save(&state)
    }
    async fn owned_project(&self, journal: &Journal, name: &str) -> Result<String, RailwayError> {
        let receipt = journal.receipt()?;
        let effect = journal.effect("project-create", digest(&(name, &receipt))?)?;
        let state = journal.load()?;
        let rows = self.projects_inventory().await?;
        let matches: Vec<_> = rows
            .iter()
            .filter(|v| v.get("description").and_then(Value::as_str) == Some(&receipt))
            .collect();
        let id = if let Some(id) = effect.id {
            id
        } else if effect.submitted {
            if matches.len() != 1 {
                return Err(api_failed(
                    "projectCreate",
                    "submitted project has no unique receipt; refusing resubmission",
                ));
            }
            let id = scalar_id(matches[0], "/id", "project")?.to_owned();
            journal.identify("project-create", &id)?;
            id
        } else {
            if !matches.is_empty()
                || rows
                    .iter()
                    .any(|v| v.get("name").and_then(Value::as_str) == Some(name))
            {
                return Err(api_failed(
                    "projectCreate",
                    "project existed before submission",
                ));
            }
            journal.submit("project-create")?;
            let data=self.graphql("projectCreate",r#"mutation ProjectCreate($input:ProjectCreateInput!){projectCreate(input:$input){id}}"#,json!({"input":{"name":name,"description":receipt,"defaultEnvironmentName":"production"}})).await?;
            let id = scalar_id(&data, "/projectCreate/id", "projectCreate")?.to_owned();
            journal.identify("project-create", &id)?;
            id
        };
        let data=self.graphql("project",r#"query OwnedProject($id:String!){project(id:$id){id name description workspaceId deletedAt}}"#,json!({"id":id})).await?;
        let node = &data["project"];
        if scalar_id(node, "/id", "project")? != id || Self::verify_project(node, &state)? {
            return Err(api_failed(
                "project",
                "project identity differs or project deleted",
            ));
        }
        journal.project(&id, nullable_id(node, "workspaceId")?.as_deref())?;
        journal.confirm("project-create")?;
        Ok(id)
    }
    async fn project_children(
        &self,
        project: &str,
        kind: &str,
    ) -> Result<Vec<Value>, RailwayError> {
        let query = match kind {
            "environments" => {
                r#"query OwnedEnvironments($id:String!,$after:String){project(id:$id){id environments(first:100,after:$after){edges{node{id name projectId canAccess deletedAt}}pageInfo{hasNextPage endCursor}}}}"#
            }
            _ => {
                r#"query OwnedServices($id:String!,$after:String){project(id:$id){id services(first:100,after:$after){edges{node{id name projectId}}pageInfo{hasNextPage endCursor}}}}"#
            }
        };
        let mut result = Vec::new();
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        let mut cursors = BTreeSet::new();
        loop {
            let data = self
                .graphql(kind, query, json!({"id":project,"after":cursor}))
                .await?;
            if scalar_id(&data, "/project/id", kind)? != project {
                return Err(api_failed(kind, "parent differs"));
            }
            let (nodes, next) = page(&data["project"][kind], &mut seen)?;
            for node in &nodes {
                if scalar_id(node, "/projectId", kind)? != project {
                    return Err(api_failed(kind, "child parent differs"));
                }
                field(node, "name")?;
            }
            result.extend(nodes);
            if let Some(next) = next {
                if !cursors.insert(next.clone()) {
                    return Err(api_failed(kind, "pagination cursor repeated"));
                }
                cursor = Some(next);
            } else {
                return Ok(result);
            }
        }
    }
    async fn owned_environment(
        &self,
        journal: &Journal,
        project: &str,
    ) -> Result<String, RailwayError> {
        let rows = self.project_children(project, "environments").await?;
        let matches: Vec<_> = rows
            .iter()
            .filter(|v| v.get("name").and_then(Value::as_str) == Some("production"))
            .collect();
        if matches.len() != 1
            || deleted(matches[0])?
            || matches[0].get("canAccess").and_then(Value::as_bool) != Some(true)
        {
            return Err(api_failed(
                "environments",
                "unique accessible production environment required",
            ));
        }
        let id = scalar_id(matches[0], "/id", "environments")?.to_owned();
        journal.environment(&id)?;
        Ok(id)
    }
    async fn variables(
        &self,
        project: &str,
        service: &str,
        environment: &str,
    ) -> Result<Value, RailwayError> {
        let data=self.graphql("variables",r#"query Variables($projectId:String!,$serviceId:String!,$environmentId:String!){variables(projectId:$projectId,serviceId:$serviceId,environmentId:$environmentId,unrendered:true)}"#,json!({"projectId":project,"serviceId":service,"environmentId":environment})).await?;
        data.get("variables")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(|| api_failed("variables", "missing variables"))
    }
    async fn owned_service(
        &self,
        journal: &Journal,
        project: &str,
        environment: &str,
        name: &str,
    ) -> Result<String, RailwayError> {
        let receipt = journal.receipt()?;
        let effect = journal.effect(
            "service-create",
            digest(&(project, environment, name, &receipt))?,
        )?;
        let rows = self.project_children(project, "services").await?;
        let matches: Vec<_> = rows
            .iter()
            .filter(|v| v.get("name").and_then(Value::as_str) == Some(name))
            .collect();
        let id = if let Some(id) = effect.id {
            id
        } else if effect.submitted {
            let mut recovered = Vec::new();
            for node in matches {
                let id = scalar_id(node, "/id", "service")?;
                if self
                    .variables(project, id, environment)
                    .await?
                    .get(SERVICE_RECEIPT)
                    .and_then(Value::as_str)
                    == Some(&receipt)
                {
                    recovered.push(id.to_owned());
                }
            }
            if recovered.len() != 1 {
                return Err(api_failed(
                    "serviceCreate",
                    "submitted service has no unique receipt; refusing resubmission",
                ));
            }
            journal.identify("service-create", &recovered[0])?;
            recovered.remove(0)
        } else {
            if !matches.is_empty() {
                return Err(api_failed(
                    "serviceCreate",
                    "service existed before submission",
                ));
            }
            journal.submit("service-create")?;
            // Create an empty service. Root and source are applied in the recorded update.
            let data=self.graphql("serviceCreate",r#"mutation ServiceCreate($input:ServiceCreateInput!){serviceCreate(input:$input){id}}"#,json!({"input":{"name":name,"projectId":project,"environmentId":environment,"variables":{SERVICE_RECEIPT:receipt}}})).await?;
            let id = scalar_id(&data, "/serviceCreate/id", "serviceCreate")?.to_owned();
            journal.identify("service-create", &id)?;
            id
        };
        let data = self
            .graphql(
                "service",
                r#"query OwnedService($id:String!){service(id:$id){id name projectId}}"#,
                json!({"id":id}),
            )
            .await?;
        let node = &data["service"];
        if scalar_id(node, "/id", "service")? != id
            || scalar_id(node, "/projectId", "service")? != project
            || field(node, "name")? != name
            || self
                .variables(project, &id, environment)
                .await?
                .get(SERVICE_RECEIPT)
                .and_then(Value::as_str)
                != Some(&receipt)
        {
            return Err(api_failed("service", "service ownership differs"));
        }
        journal.service(&id)?;
        journal.confirm("service-create")?;
        Ok(id)
    }
    async fn configure_journaled(
        &self,
        journal: &Journal,
        project: &str,
        service: &str,
        environment: &str,
        source: &ServiceSource,
        variables: &BTreeMap<String, String>,
    ) -> Result<(), RailwayError> {
        let (source, command, root) = match source {
            ServiceSource::Image {
                image,
                start_command,
            } => (
                json!({"image":image,"repo":null}),
                json!(start_command),
                "/".to_owned(),
            ),
            ServiceSource::GitHubRepo { repo, root, .. } => (
                json!({"repo":repo,"image":null}),
                Value::Null,
                format!(
                    "/{}",
                    root.as_deref()
                        .unwrap_or("")
                        .trim_matches('/')
                        .trim_start_matches("./")
                ),
            ),
        };
        let input = json!({"source":source,"rootDirectory":root,"startCommand":command});
        let key = journal.revision_key("settings");
        let effect = journal.effect(&key, digest(&(service, environment, &input))?)?;
        let matches = |instance: &Value| {
            instance.get("source") == Some(&source)
                && instance.get("startCommand") == Some(&command)
                && instance
                    .get("rootDirectory")
                    .and_then(Value::as_str)
                    .map(|s| s.trim_matches('/').trim_start_matches("./"))
                    == Some(root.trim_matches('/'))
        };
        if !matches(&self.service_instance(service, environment).await?) {
            if effect.submitted {
                return Err(api_failed(
                    "settings",
                    "recorded settings mutation remains unapplied or has drifted",
                ));
            }
            journal.submit(&key)?;
            let data=self.graphql("serviceInstanceUpdate",r#"mutation ServiceInstanceUpdate($serviceId:String!,$environmentId:String!,$input:ServiceInstanceUpdateInput!){serviceInstanceUpdate(serviceId:$serviceId,environmentId:$environmentId,input:$input)}"#,json!({"serviceId":service,"environmentId":environment,"input":input})).await?;
            if data.get("serviceInstanceUpdate").and_then(Value::as_bool) != Some(true)
                || !matches(&self.service_instance(service, environment).await?)
            {
                return Err(api_failed("settings", "settings update unconfirmed"));
            }
        }
        journal.confirm(&key)?;
        let key = journal.revision_key("variables");
        let effect = journal.effect(&key, digest(&(project, service, environment, variables))?)?;
        if self.variables(project, service, environment).await? != json!(variables) {
            if effect.submitted {
                return Err(api_failed(
                    "variables",
                    "recorded variable mutation remains unapplied or has drifted",
                ));
            }
            journal.submit(&key)?;
            let data=self.graphql("variableCollectionUpsert",r#"mutation VariableCollectionUpsert($input:VariableCollectionUpsertInput!){variableCollectionUpsert(input:$input)}"#,json!({"input":{"projectId":project,"serviceId":service,"environmentId":environment,"variables":variables,"replace":true,"skipDeploys":true}})).await?;
            if data
                .get("variableCollectionUpsert")
                .and_then(Value::as_bool)
                != Some(true)
                || self.variables(project, service, environment).await? != json!(variables)
            {
                return Err(api_failed("variables", "variables update unconfirmed"));
            }
        }
        journal.confirm(&key)
    }
    async fn domain_inventory(
        &self,
        project: &str,
        service: &str,
        environment: &str,
    ) -> Result<Vec<Value>, RailwayError> {
        let data=self.graphql("domains",r#"query Domains($projectId:String!,$serviceId:String!,$environmentId:String!){domains(projectId:$projectId,serviceId:$serviceId,environmentId:$environmentId){serviceDomains{id domain projectId serviceId environmentId deletedAt}}}"#,json!({"projectId":project,"serviceId":service,"environmentId":environment})).await?;
        let rows = data
            .pointer("/domains/serviceDomains")
            .and_then(Value::as_array)
            .ok_or_else(|| api_failed("domains", "inventory missing"))?;
        let mut seen = BTreeSet::new();
        let mut active = Vec::new();
        for node in rows {
            if !seen.insert(scalar_id(node, "/id", "domains")?)
                || scalar_id(node, "/projectId", "domains")? != project
                || scalar_id(node, "/serviceId", "domains")? != service
                || scalar_id(node, "/environmentId", "domains")? != environment
            {
                return Err(api_failed("domains", "domain parent or identity differs"));
            }
            if !deleted(node)? {
                active.push(node.clone());
            }
        }
        Ok(active)
    }
    async fn owned_domain(
        &self,
        journal: &Journal,
        project: &str,
        service: &str,
        environment: &str,
    ) -> Result<String, RailwayError> {
        let key = "domain-create";
        let effect = journal.effect(key, digest(&(project, service, environment))?)?;
        let mut rows = self.domain_inventory(project, service, environment).await?;
        if rows.is_empty() && !effect.submitted {
            journal.submit(key)?;
            let data=self.graphql("serviceDomainCreate",r#"mutation ServiceDomainCreate($input:ServiceDomainCreateInput!){serviceDomainCreate(input:$input){id domain}}"#,json!({"input":{"serviceId":service,"environmentId":environment}})).await?;
            journal.identify(
                key,
                scalar_id(&data, "/serviceDomainCreate/id", "serviceDomainCreate")?,
            )?;
            rows = self.domain_inventory(project, service, environment).await?;
        }
        if rows.len() != 1 {
            return Err(api_failed(
                "domains",
                "owned service has no unique domain; refusing creation",
            ));
        }
        let id = scalar_id(&rows[0], "/id", "domains")?;
        let current = journal.load()?.effects[key].clone();
        if current.submitted {
            journal.identify(key, id)?;
        } else {
            return Err(api_failed("domains", "domain existed before submission"));
        }
        journal.confirm(key)?;
        let domain = field(&rows[0], "domain")?.to_owned();
        let candidate = if domain.contains("://") {
            domain.clone()
        } else {
            format!("https://{domain}")
        };
        let url = reqwest::Url::parse(&candidate).map_err(|e| api_failed("domains", e))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(api_failed("domains", "invalid origin"));
        }
        Ok(url.as_str().trim_end_matches('/').to_owned())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn deploy_journaled(
        &self,
        journal: &Journal,
        project_name: &str,
        service_name: &str,
        source: &ServiceSource,
        mut variables: BTreeMap<String, String>,
        service: &str,
        budget: Duration,
    ) -> Result<DeployOutcome, RailwayError> {
        journal.initialize(project_name, service_name)?;
        let deploy_key = journal.revision_key("deploy");
        if journal.load()?.effects.iter().any(|(key, e)| {
            key.ends_with(":deploy") && key != &deploy_key && e.submitted && e.id.is_none()
        }) {
            return Err(api_failed(
                "deploy",
                "previous deployment submission unresolved",
            ));
        }
        if variables.contains_key(SERVICE_RECEIPT) {
            return Err(api_failed(
                "variables",
                "reserved Railway service receipt variable",
            ));
        }
        variables.insert(SERVICE_RECEIPT.into(), journal.receipt()?);
        let project_id = self.owned_project(journal, project_name).await?;
        let environment_id = self.owned_environment(journal, &project_id).await?;
        let service_id = self
            .owned_service(journal, &project_id, &environment_id, service_name)
            .await?;
        self.configure_journaled(
            journal,
            &project_id,
            &service_id,
            &environment_id,
            source,
            &variables,
        )
        .await?;
        let origin = self
            .owned_domain(journal, &project_id, &service_id, &environment_id)
            .await?;
        let commit = match source {
            ServiceSource::GitHubRepo { commit_sha, .. } => Some(commit_sha.as_str()),
            _ => None,
        };
        let effect = journal.effect(
            &deploy_key,
            digest(&(
                &project_id,
                &environment_id,
                &service_id,
                commit,
                &variables,
            ))?,
        )?;
        let deployment_id = if let Some(id) = effect.id {
            id
        } else {
            journal.submit(&deploy_key)?;
            let id = self
                .trigger_deploy(&service_id, &environment_id, commit)
                .await?;
            journal.identify(&deploy_key, &id)?;
            id
        };
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let state = self.deployment_status(&deployment_id).await?;
            if state.is_failed() {
                return Err(RailwayError::DeployFailed {
                    service: service.into(),
                    state: state.as_str().into(),
                });
            }
            if self
                .deployment_revision_ready(
                    &project_id,
                    &service_id,
                    &environment_id,
                    &deployment_id,
                    commit,
                )
                .await?
            {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(api_failed(
                    "deploy",
                    "recorded deployment did not become active within budget",
                ));
            }
            tokio::time::sleep(self.poll_interval).await;
        }
        journal.confirm(&deploy_key)?;
        Ok(DeployOutcome {
            project_id,
            environment_id,
            service_id,
            deployment_id,
            domain: origin.clone(),
            origin,
        })
    }
}
